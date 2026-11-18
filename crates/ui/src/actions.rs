//! App-wide actions and key bindings.

use crate::state::state;
use gpui::{actions, App, KeyBinding};

actions!(zwisper, [Quit, OpenLibrary, OpenSettings, StartMeeting, CloseWindow]);

pub fn bind(cx: &mut App) {
    let m = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{m}-q"), Quit, None),
        KeyBinding::new(&format!("{m}-,"), OpenSettings, None),
        KeyBinding::new(&format!("{m}-l"), OpenLibrary, None),
        KeyBinding::new(&format!("{m}-w"), CloseWindow, None),
    ]);
    cx.on_action(|_: &Quit, cx| {
        state(cx).read(cx).send(zwisper_core::Command::Shutdown);
        cx.quit()
    });
    cx.on_action(|_: &OpenLibrary, cx| open_library(cx));
    cx.on_action(|_: &OpenSettings, cx| open_settings(cx));
    cx.on_action(|_: &StartMeeting, cx| start_meeting(cx));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
    });
}

pub fn open_library(cx: &mut App) {
    state(cx).update(cx, |s, cx| s.open_library(cx));
}

pub fn open_settings(cx: &mut App) {
    state(cx).update(cx, |s, cx| s.open_settings(cx));
}

/// Start a meeting, or bring the live one forward.
pub fn start_meeting(cx: &mut App) {
    let st = state(cx);
    let live = st.read(cx).meeting.as_ref().map(|m| m.id);
    match live {
        Some(id) => st.update(cx, |s, cx| s.open_meeting(id, cx)),
        None => st.read(cx).send(zwisper_core::Command::StartMeeting { title: None }),
    }
}

pub fn toggle_dictation(cx: &mut App) {
    let st = state(cx);
    let idle = matches!(st.read(cx).dictation, zwisper_core::DictationPhase::Idle
            | zwisper_core::DictationPhase::Error(_)
            | zwisper_core::DictationPhase::Copied { .. }
    );
    st.read(cx).send(if idle { zwisper_core::Command::StartDictation } else { zwisper_core::Command::StopDictation });
}
