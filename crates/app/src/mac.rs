//! macOS composition: real storage, models, engines and platform adapters,
//! plus the menu-bar icon and global shortcuts.

use crate::engines::RealEngines;
use gpui::App;
use std::path::Path;
use std::sync::Arc;
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
        }),
    );
    if let Err(e) = res {
        tracing::error!(%e, "could not bind shortcuts");
    }
    // Keep the manager alive for the life of the app.
    Box::leak(Box::new(hk));
}
