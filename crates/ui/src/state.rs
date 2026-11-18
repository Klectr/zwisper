//! `AppState`: the UI's view-model. Bridges core `Event`s onto the GPUI thread,
//! keeps light-weight live state, and owns window lifecycle.

use crate::theme::Theme;
use crate::{library, meeting, onboarding, pill, settings};
use gpui::{AnyWindowHandle, App, AppContext, Context, Entity, EventEmitter, Global, WindowHandle};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zwisper_core::domain::*;
use zwisper_core::{Command, Core, DictationPhase, Event};

/// Platform hooks the UI needs that are not core ports (window chrome,
/// activation policy, login item). Implemented by the composition root.
pub trait UiPlatform: Send + Sync {
    /// Show/hide the app in the Dock and ⌘Tab (macOS accessory vs regular).
    fn set_regular_app(&self, _regular: bool) {}
    fn set_launch_at_login(&self, _on: bool) -> zwisper_core::Result<()> {
        Ok(())
    }
    fn permissions(&self) -> Arc<dyn zwisper_core::ports::Permissions>;
    /// Strip OS window chrome (frame, corner mask, shadow) from an overlay
    /// window so only its own rounded content shows, e.g. the dictation pill.
    fn make_frameless(&self, _window: &gpui::Window) {}
}

#[derive(Clone)]
pub enum UiEvent {
    Session(Box<Session>),
    Live(SessionId, TranscriptSegment),
    NotesStarted(SessionId, Template),
    NotesDelta(SessionId, Template, String),
    NotesDone(SessionId, Template),
    Deleted(SessionId),
}

pub struct LiveMeeting {
    pub id: SessionId,
    pub state: SessionState,
    pub elapsed_ms: Ms,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct AppState {
    pub core: Core,
    pub platform: Arc<dyn UiPlatform>,
    pub dictation: DictationPhase,
    pub levels: Vec<f32>,
    pub meeting: Option<LiveMeeting>,
    pub meeting_levels: HashMap<TrackKind, f32>,
    pub pipeline: HashMap<SessionId, String>,
    pub packs: HashMap<Pack, PackStatus>,
    pub settings: Settings,
    pub tier: Tier,
    pub toast: Option<Toast>,
    pub library_version: u64,
    pill: Option<WindowHandle<pill::Pill>>,
    pill_hide_at: Option<Instant>,
    meetings: HashMap<SessionId, AnyWindowHandle>,
    library: Option<AnyWindowHandle>,
    settings_win: Option<AnyWindowHandle>,
    onboarding: Option<AnyWindowHandle>,
}

impl EventEmitter<UiEvent> for AppState {}

pub struct UiGlobal(pub Entity<AppState>);
impl Global for UiGlobal {}

pub fn state(cx: &App) -> Entity<AppState> {
    cx.global::<UiGlobal>().0.clone()
}

pub const LEVEL_HISTORY: usize = 56;

impl AppState {
    pub fn new(core: Core, platform: Arc<dyn UiPlatform>) -> Self {
        let tier = core.tier();
        let packs = Pack::ALL.iter().map(|p| (*p, core.pack_status(*p))).collect();
        AppState {
            settings: core.settings(),
            tier,
            packs,
            core,
            platform,
            dictation: DictationPhase::Idle,
            levels: vec![0.0; LEVEL_HISTORY],
            meeting: None,
            meeting_levels: HashMap::new(),
            pipeline: HashMap::new(),
            toast: None,
            library_version: 0,
            pill: None,
            pill_hide_at: None,
            meetings: HashMap::new(),
            library: None,
            settings_win: None,
            onboarding: None,
        }
    }

    pub fn send(&self, cmd: Command) {
        self.core.send(cmd);
    }

    /// Start pumping core events into this entity.
    pub fn attach(entity: &Entity<AppState>, cx: &mut App) {
        let rx = entity.read(cx).core.subscribe();
        let (tx, arx) = async_channel::unbounded::<Event>();
        std::thread::Builder::new()
            .name("ui-event-bridge".into())
            .spawn(move || {
                while let Ok(e) = rx.recv() {
                    if tx.send_blocking(e).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn bridge");
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(e) = arx.recv().await {
                if weak.update(cx, |s, cx| s.handle(e, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Housekeeping tick (toast expiry, pill fade) — only runs while something is visible.
        let weak = entity.downgrade();
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if weak.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = &self.toast {
            if t.at.elapsed() > Duration::from_secs(if t.error { 6 } else { 4 }) {
                self.toast = None;
                cx.notify();
            }
        }
        if let Some(at) = self.pill_hide_at {
            if Instant::now() >= at {
                self.pill_hide_at = None;
                self.close_pill(cx);
            }
        }
    }

    pub fn notify_toast(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.toast = Some(Toast { text: text.into(), error, at: Instant::now() });
        cx.notify();
    }

    fn handle(&mut self, e: Event, cx: &mut Context<Self>) {
        match e {
            Event::Dictation(phase) => {
                match &phase {
                    DictationPhase::Listening => {
                        self.levels = vec![0.0; LEVEL_HISTORY];
                        self.pill_hide_at = None;
                        self.open_pill(cx);
                    }
                    DictationPhase::Transcribing => {}
                    DictationPhase::Idle => self.pill_hide_at = Some(Instant::now() + Duration::from_millis(150)),
                    DictationPhase::Error(_) => {
                        self.open_pill(cx);
                        self.pill_hide_at = Some(Instant::now() + Duration::from_secs(3));
                    }
                    // Longer: the user has to read it and may click "Allow…".
                    DictationPhase::Copied { .. } => {
                        self.open_pill(cx);
                        self.pill_hide_at = Some(Instant::now() + Duration::from_secs(8));
                    }
                }
                self.dictation = phase;
            }
            Event::DictationLevel(l) => {
                self.levels.remove(0);
                self.levels.push(l);
            }
            Event::Dictated(_) => {}
            Event::MeetingStarted(id) => {
                self.meeting = Some(LiveMeeting { id, state: SessionState::Recording, elapsed_ms: 0 });
                self.platform.set_regular_app(true);
                self.open_meeting(id, cx);
            }
            Event::MeetingState { id, state } => {
                if let Some(m) = self.meeting.as_mut().filter(|m| m.id == id) {
                    m.state = state;
                    if matches!(state, SessionState::Processing | SessionState::Done | SessionState::Failed) {
                        self.meeting = None;
                        self.meeting_levels.clear();
                    }
                }
                if matches!(state, SessionState::Done | SessionState::Failed) {
                    self.pipeline.remove(&id);
                }
            }
            Event::MeetingLevel { track, level } => {
                self.meeting_levels.insert(track, level);
            }
            Event::MeetingElapsed { id, ms } => {
                if let Some(m) = self.meeting.as_mut().filter(|m| m.id == id) {
                    m.elapsed_ms = ms;
                }
            }
            Event::LiveSegment { id, segment } => cx.emit(UiEvent::Live(id, segment)),
            Event::Pipeline { id, step, done } => {
                if done {
                    self.pipeline.remove(&id);
                } else {
                    self.pipeline.insert(id, step);
                }
            }
            Event::SessionUpdated(s) => cx.emit(UiEvent::Session(s)),
            Event::SessionDeleted(id) => {
                if let Some(w) = self.meetings.remove(&id) {
                    let _ = w.update(cx, |_, window, _| window.remove_window());
                }
                cx.emit(UiEvent::Deleted(id));
                self.library_version += 1;
            }
            Event::LibraryChanged => self.library_version += 1,
            Event::NotesStarted { id, template } => cx.emit(UiEvent::NotesStarted(id, template)),
            Event::NotesDelta { id, template, delta } => cx.emit(UiEvent::NotesDelta(id, template, delta)),
            Event::NotesDone { id, template } => cx.emit(UiEvent::NotesDone(id, template)),
            Event::Pack { pack, status } => {
                self.packs.insert(pack, status);
            }
            Event::TierChanged(t) => {
                self.tier = t;
                for p in Pack::ALL {
                    self.packs.insert(p, self.core.pack_status(p));
                }
            }
            Event::SettingsChanged(s) => self.settings = s,
            Event::Notice(text) => self.notify_toast(text, false, cx),
            Event::Error(text) => self.notify_toast(text, true, cx),
        }
        cx.notify();
    }

    // ───── windows ─────

    fn open_pill(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.pill {
            if p.update(cx, |_, _, _| ()).is_ok() {
                return;
            }
        }
        // Windows read AppState while being built, so open them after this update.
        cx.defer(|cx| {
            let h = pill::open(cx);
            state(cx).update(cx, |s, _| s.pill = h);
        });
    }

    fn close_pill(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.pill.take() {
            let _ = p.update(cx, |_, window, _| window.remove_window());
        }
    }

    pub fn open_meeting(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let Some(w) = self.meetings.get(&id) {
            if w.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }
        cx.defer(move |cx| {
            if let Some(w) = meeting::open(id, cx) {
                state(cx).update(cx, |s, _| {
                    s.meetings.insert(id, w);
                });
            }
        });
    }

    pub fn meeting_closed(&mut self, id: SessionId) {
        self.meetings.remove(&id);
        self.update_activation();
    }

    pub fn open_library(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = &self.library {
            if w.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }
        cx.defer(|cx| {
            let h = library::open(cx);
            state(cx).update(cx, |s, _| s.library = h);
        });
        self.platform.set_regular_app(true);
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = &self.settings_win {
            if w.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }
        cx.defer(|cx| {
            let h = settings::open(cx);
            state(cx).update(cx, |s, _| s.settings_win = h);
        });
        self.platform.set_regular_app(true);
    }

    pub fn open_onboarding(&mut self, cx: &mut Context<Self>) {
        cx.defer(|cx| {
            let h = onboarding::open(cx);
            state(cx).update(cx, |s, _| s.onboarding = h);
        });
        self.platform.set_regular_app(true);
    }

    pub fn window_closed(&mut self, which: &'static str) {
        match which {
            "library" => self.library = None,
            "settings" => self.settings_win = None,
            "onboarding" => self.onboarding = None,
            _ => {}
        }
        self.update_activation();
    }

    fn update_activation(&self) {
        let any = self.library.is_some() || self.settings_win.is_some() || self.onboarding.is_some() || !self.meetings.is_empty();
        self.platform.set_regular_app(any);
    }

    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.clone();
        f(&mut s);
        self.settings = s.clone();
        self.send(Command::UpdateSettings(s));
    }
}

/// Install the UI: theme, key bindings, state. Call once from the app's main.
/// Catppuccin from GPUI Kit's theme collection (gpui-kit.com/themes):
/// Mocha when dark, Latte when light.
const THEMES: &str = include_str!("../themes/catppuccin.json");
const DARK_THEME: &str = "Catppuccin Mocha";
const LIGHT_THEME: &str = "Catppuccin Latte";

/// Load the kit theme for `dark`/light and return our palette derived from it,
/// so custom widgets (pill, cards, keycaps…) match the kit's components.
fn load_theme(dark: bool, cx: &mut App) -> Theme {
    use gpui::component::{Theme as Kit, ThemeMode, ThemeRegistry};
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEMES) {
        tracing::warn!(error = %e, "could not load the Catppuccin themes; using the kit default");
    }
    let themes = ThemeRegistry::global(cx).themes();
    let (mocha, latte) = (themes.get(DARK_THEME).cloned(), themes.get(LIGHT_THEME).cloned());
    Kit::update(cx, |k| {
        if let Some(t) = mocha {
            k.dark_theme = t;
        }
        if let Some(t) = latte {
            k.light_theme = t;
        }
    });
    // `change` applies the registered theme for that mode.
    Kit::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::from_kit(&Kit::global(cx).colors, dark)
}

pub fn install(core: Core, platform: Arc<dyn UiPlatform>, cx: &mut App) -> Entity<AppState> {
    let dark = match std::env::var("ZWISPER_THEME").as_deref() {
        Ok("dark") => true,
        Ok("light") => false,
        _ => !matches!(cx.window_appearance(), gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight),
    };
    gpui::init(cx);
    let theme = load_theme(dark, cx);
    cx.set_global(theme);
    crate::editor::bind_keys(cx);
    crate::actions::bind(cx);
    let entity = cx.new(|_| AppState::new(core, platform));
    cx.set_global(UiGlobal(entity.clone()));
    AppState::attach(&entity, cx);
    let onboarded = entity.read(cx).settings.onboarded;
    entity.update(cx, |s, cx| {
        if !onboarded {
            s.open_onboarding(cx);
        }
    });
    entity
}
