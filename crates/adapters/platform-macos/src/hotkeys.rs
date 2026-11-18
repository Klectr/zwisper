//! `HotkeyService` via the `global-hotkey` crate (Carbon `RegisterEventHotKey`).

use crate::chord::{parse_chord, RepeatFilter};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use zwisper_core::ports::{HotkeyAction, HotkeyCallback, HotkeyService, KeyState};
use zwisper_core::{Error, Result};

struct Bindings {
    actions: HashMap<u32, HotkeyAction>,
    callback: Option<Arc<HotkeyCallback>>,
    repeat: RepeatFilter,
}

/// Global shortcuts with key-down **and** key-up delivery (for push-to-talk).
///
/// Threading:
/// * `MacHotkeys::new()` **must be called on the main thread** (Carbon event
///   handlers are installed on the main event loop); it returns
///   `Error::InvalidState` otherwise. The app must be running an
///   `NSApplication` run loop (GPUI does) for events to arrive.
/// * `bind`/`unbind_all` may be called from any thread afterwards.
/// * The callback runs on a dedicated `zwisper-hotkeys` thread, never the main
///   thread. Keep it short (e.g. send a `Command`).
///
/// Only one `MacHotkeys` should exist per process: `global-hotkey` delivers
/// all events through one global channel.
pub struct MacHotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    shared: Arc<Mutex<Bindings>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MacHotkeys {
    pub fn new() -> Result<Self> {
        if MainThreadMarker::new().is_none() {
            return Err(Error::InvalidState("MacHotkeys::new must be called on the main thread".into()));
        }
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| Error::InvalidState(format!("global hotkeys unavailable: {e}")))?;
        let shared = Arc::new(Mutex::new(Bindings {
            actions: HashMap::new(),
            callback: None,
            repeat: RepeatFilter::default(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("zwisper-hotkeys".into())
                .spawn(move || listen(shared, stop))
                .map_err(|e| Error::InvalidState(format!("spawn hotkey thread: {e}")))?
        };
        Ok(MacHotkeys { manager, registered: Vec::new(), shared, stop, thread: Some(thread) })
    }
}

fn listen(shared: Arc<Mutex<Bindings>>, stop: Arc<AtomicBool>) {
    let rx = GlobalHotKeyEvent::receiver();
    while !stop.load(Ordering::Relaxed) {
        let ev = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ev) => ev,
            Err(e) if e.is_timeout() => continue,
            Err(_) => break,
        };
        let pressed = ev.state() == HotKeyState::Pressed;
        // Resolve under the lock, call the callback outside it.
        let hit = {
            let mut b = match shared.lock() {
                Ok(b) => b,
                Err(p) => p.into_inner(),
            };
            match (b.actions.get(&ev.id()).copied(), b.callback.clone()) {
                (Some(action), Some(cb)) if b.repeat.accept(ev.id(), pressed) => Some((action, cb)),
                _ => None,
            }
        };
        if let Some((action, cb)) = hit {
            let state = if pressed { KeyState::Pressed } else { KeyState::Released };
            cb(action, state);
        }
    }
}

impl HotkeyService for MacHotkeys {
    fn bind(&mut self, bindings: &[(HotkeyAction, String)], callback: HotkeyCallback) -> Result<()> {
        // Parse everything first so a bad chord leaves the old bindings intact.
        let mut parsed = Vec::with_capacity(bindings.len());
        for (action, chord) in bindings {
            let c = parse_chord(chord).map_err(|e| Error::InvalidState(format!("shortcut: {e}")))?;
            let hk = HotKey::from_str(&c.to_hotkey_string())
                .map_err(|e| Error::InvalidState(format!("shortcut '{chord}': {e}")))?;
            if parsed.iter().any(|(_, h): &(HotkeyAction, HotKey)| h.id() == hk.id()) {
                return Err(Error::InvalidState(format!("shortcut '{chord}' is bound twice")));
            }
            parsed.push((*action, hk));
        }
        self.unbind_all();
        let mut actions = HashMap::new();
        for (action, hk) in &parsed {
            if let Err(e) = self.manager.register(*hk) {
                self.unbind_all();
                return Err(Error::InvalidState(format!(
                    "could not register shortcut {}: {e} (it may be used by another app)",
                    hk.into_string()
                )));
            }
            self.registered.push(*hk);
            actions.insert(hk.id(), *action);
        }
        let mut b = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        b.actions = actions;
        b.callback = Some(Arc::new(callback));
        b.repeat.clear();
        Ok(())
    }

    fn unbind_all(&mut self) {
        if !self.registered.is_empty() {
            if let Err(e) = self.manager.unregister_all(&self.registered) {
                tracing::warn!(error = %e, "unregistering hotkeys failed");
            }
            self.registered.clear();
        }
        let mut b = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        b.actions.clear();
        b.callback = None;
        b.repeat.clear();
    }
}

impl Drop for MacHotkeys {
    fn drop(&mut self) {
        self.unbind_all();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chord the settings UI can produce must be accepted by global-hotkey.
    #[test]
    fn canonical_strings_parse_in_global_hotkey() {
        for c in ["Alt+Space", "Alt+Shift+M", "Cmd+Ctrl+1", "F13", "⌘⇧K", "Ctrl+Escape", "Alt+/"] {
            let s = parse_chord(c).unwrap().to_hotkey_string();
            HotKey::from_str(&s).unwrap_or_else(|e| panic!("{c} -> {s}: {e}"));
        }
        assert_eq!(
            HotKey::from_str(&parse_chord("Alt+Space").unwrap().to_hotkey_string()).unwrap(),
            HotKey::from_str("Option+Space").unwrap()
        );
    }
}
