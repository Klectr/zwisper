//! macOS composition: real storage, models, engines and platform adapters,
//! plus the menu-bar icon and global shortcuts.

use crate::engines::RealEngines;
use gpui::App;
use std::path::Path;
use std::sync::Arc;
use zwisper_core::domain::{PermissionKind, PermissionState};
use zwisper_core::ports::*;
use zwisper_core::{Command, Core, Ports};
use zwisper_platform_macos as platform;

pub struct MacUi {
    perms: Arc<platform::MacPermissions>,
}

impl zwisper_ui::UiPlatform for MacUi {
    fn set_regular_app(&self, regular: bool) {
        let _ = platform::set_activation_policy_accessory(!regular);
        if regular {
            let _ = platform::activate_app();
        }
    }
    fn set_launch_at_login(&self, on: bool) -> zwisper_core::Result<()> {
        platform::set_launch_at_login(on)
    }
    fn permissions(&self) -> Arc<dyn Permissions> {
        self.perms.clone()
    }
    fn make_frameless(&self, window: &gpui::Window) {
        make_frameless(window)
    }
}

pub fn make_frameless(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(h) = handle.as_raw() {
        // SAFETY: GPUI's AppKit handle is its live NSView, and we're on the main thread.
        if let Err(e) = unsafe { platform::make_view_window_frameless_later(h.ns_view.as_ptr()) } {
            tracing::warn!(%e, "could not make window frameless");
        }
    }
}

pub fn compose(cx: &mut App, data: &Path) -> (Core, Arc<dyn zwisper_ui::UiPlatform>) {
    let settings = Arc::new(zwisper_store_fs::TomlSettings::new(data.join("config.toml")));
    let mut s = settings.load();
    let library = s.library_dir.clone().unwrap_or_else(|| {
        directories::UserDirs::new().map(|u| u.home_dir().join("Zwisper")).unwrap_or_else(|| data.join("Library"))
    });
    if s.library_dir.is_none() {
        s.library_dir = Some(library.clone());
        let _ = settings.save(&s);
    }
    let store = Arc::new(zwisper_store_fs::FsStore::new(&library).expect("library folder"));
    let models: Arc<dyn ModelStore> = Arc::new(zwisper_models_http::HttpModelStore::new(data.join("models")));

    let ports = Ports {
        audio: Arc::new(zwisper_audio_macos::MacAudio::new()),
        recorder: store.clone(),
        injector: Arc::new(zwisper_core::lazy::Fallback {
            primary: platform::PasteInjector::new(),
            secondary: platform::TypingInjector::new(),
        }),
        ducker: Arc::new(zwisper_audio_macos::MacDucker::new(
            data.join("ducking.state"),
            Box::new(|| platform::MacPermissions::new().state(PermissionKind::SystemAudio) == PermissionState::Granted),
        )),
        repo: store,
        settings,
        models: models.clone(),
        templates: Arc::new(zwisper_templates::BuiltinTemplates),
        aligner: Arc::new(zwisper_core::align::MaxOverlapAligner),
        echo: Arc::new(zwisper_core::echo::TextSimilarityEcho::default()),
        clock: Arc::new(platform::SystemClock),
        engines: Arc::new(RealEngines { models }),
        system_memory_bytes: platform::system_memory_bytes(),
    };
    let core = Core::start(ports).expect("start core");

    install_hotkeys(&core);
    crate::tray::install(cx);
    let _ = platform::set_activation_policy_accessory(true);

    (core, Arc::new(MacUi { perms: Arc::new(platform::MacPermissions::new()) }))
}

/// Global shortcuts → commands. Must run on the main thread.
fn install_hotkeys(core: &Core) {
    let mut hk = match platform::MacHotkeys::new() {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(%e, "global shortcuts unavailable");
            return;
        }
    };
    let s = core.settings();
    let c = core.clone();
    let events = core.subscribe();
    let res = hk.bind(
        &[(HotkeyAction::Dictate, s.dictate_shortcut.clone()), (HotkeyAction::Meeting, s.meeting_shortcut.clone())],
        Box::new(move |action, key| match action {
            HotkeyAction::Dictate => c.send(Command::DictationHotkey(key)),
            HotkeyAction::Meeting if key == KeyState::Pressed => {
                if c.active_meeting().is_none() {
                    c.send(Command::StartMeeting { title: None });
                }
            }
            HotkeyAction::Meeting => {}
            HotkeyAction::Cancel if key == KeyState::Pressed => c.send(Command::CancelDictation),
            HotkeyAction::Cancel => {}
        }),
    );
    if let Err(e) = res {
        tracing::error!(%e, "could not bind shortcuts");
    }
    // Esc cancels, but only while listening: a global Esc would otherwise be
    // taken from every other app. (The pill never has focus, so a normal key
    // binding can't see it.)
    let hk = std::sync::Arc::new(std::sync::Mutex::new(hk));
    let watcher = hk.clone();
    let spawned = std::thread::Builder::new().name("zwisper-esc".into()).spawn(move || {
        let mut bound = false;
        for e in events {
            let zwisper_core::Event::Dictation(phase) = e else { continue };
            let want = matches!(phase, zwisper_core::DictationPhase::Listening);
            if want != bound {
                let mut hk = watcher.lock().unwrap_or_else(|p| p.into_inner());
                match hk.set_temporary(HotkeyAction::Cancel, want.then_some("Escape")) {
                    Ok(()) => bound = want,
                    Err(e) => tracing::warn!(%e, "could not toggle Esc-to-cancel"),
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(%e, "Esc-to-cancel unavailable");
    }
    // Keep the manager alive for the life of the app.
    std::mem::forget(hk);
}
