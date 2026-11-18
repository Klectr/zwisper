//! `TextInjector` strategies: pasteboard + ⌘V (primary) and CGEvent unicode
//! typing (fallback). Combine with `zwisper_core::lazy::Fallback`.

use crate::permissions::accessibility_trusted;
use crate::typing::utf16_chunks;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation};
use objc2_foundation::{NSArray, NSData, NSString};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;
use zwisper_core::domain::PermissionKind;
use zwisper_core::ports::{EngineId, TextInjector};
use zwisper_core::{Error, Result};

/// `kVK_ANSI_V` from <HIToolbox/Events.h>.
const KEY_V: u16 = 9;

/// Marker type from nspasteboard.org: clipboard managers skip transient items,
/// so dictated text doesn't pollute the user's clipboard history.
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// The previous clipboard restore, joined before the next paste so two quick
/// dictations can't interleave save/restore.
static PENDING_RESTORE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// A deep copy of the pasteboard's items (every type's data).
struct Snapshot(Vec<Retained<NSPasteboardItem>>);
// SAFETY: the items are freshly created, owned exclusively by the snapshot and
// only touched by one thread at a time (moved into the restore thread).
unsafe impl Send for Snapshot {}

fn snapshot(pb: &NSPasteboard) -> Snapshot {
    let mut out = Vec::new();
    if let Some(items) = pb.pasteboardItems() {
        for item in items.iter() {
            let copy = NSPasteboardItem::new();
            for ty in item.types().iter() {
                // Limitation: lazily-provided / promised data (file promises,
                // data providers) is materialised here or skipped if unavailable.
                if let Some(data) = item.dataForType(&ty) {
                    copy.setData_forType(&data, &ty);
                }
            }
            out.push(copy);
        }
    }
    Snapshot(out)
}

fn restore(pb: &NSPasteboard, snap: Snapshot) {
    pb.clearContents();
    if snap.0.is_empty() {
        return;
    }
    let objs: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        snap.0.into_iter().map(ProtocolObject::from_retained).collect();
    let array = NSArray::from_retained_slice(&objs);
    if !pb.writeObjects(&array) {
        tracing::warn!("restoring the pasteboard failed");
    }
}

/// Post a key down + up with the given flags.
pub(crate) fn post_key(source: Option<&CGEventSource>, keycode: u16, flags: CGEventFlags) -> Result<()> {
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(source, keycode, down)
            .ok_or_else(|| Error::InvalidState("CGEventCreateKeyboardEvent failed".into()))?;
        CGEvent::set_flags(Some(&ev), flags);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
    }
    Ok(())
}

pub(crate) fn event_source() -> Option<objc2_core_foundation::CFRetained<CGEventSource>> {
    // CombinedSessionState: our synthetic events don't inherit the physical
    // modifier state (e.g. ⌥ still held from the dictation hotkey).
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
}

/// Paste via the general pasteboard and a synthetic ⌘V, then restore the
/// previous pasteboard contents ~150 ms later on a background thread.
///
/// * Requires **Accessibility** (`AXIsProcessTrusted`). Without it returns
///   `Error::Permission(Accessibility)` without touching the clipboard.
/// * Saves and restores *all* pasteboard items and types (deep copy), not only
///   the string. Promised/lazy data that can't be materialised is lost.
/// * The restore is skipped if something else changed the pasteboard in the
///   meantime (change-count check), so a user's copy is never clobbered.
/// * Callable from any thread (doesn't need the main thread).
pub struct PasteInjector {
    restore_after: Duration,
}

impl Default for PasteInjector {
    fn default() -> Self {
        PasteInjector { restore_after: Duration::from_millis(150) }
    }
}

impl PasteInjector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_restore_delay(restore_after: Duration) -> Self {
        PasteInjector { restore_after }
    }
}

impl TextInjector for PasteInjector {
    fn id(&self) -> EngineId {
        "mac-paste"
    }

    fn inject(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if !accessibility_trusted() {
            return Err(Error::Permission(PermissionKind::Accessibility));
        }
        // Finish any in-flight restore first.
        if let Some(h) = PENDING_RESTORE.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = h.join();
        }

        let pb = NSPasteboard::generalPasteboard();
        let saved = snapshot(&pb);

        pb.clearContents();
        // SAFETY: NSPasteboardTypeString is an immutable framework constant.
        let string_type = unsafe { NSPasteboardTypeString };
        if !pb.setString_forType(&NSString::from_str(text), string_type) {
            restore(&pb, saved);
            return Err(Error::InvalidState("could not write to the pasteboard".into()));
        }
        pb.setData_forType(Some(&NSData::new()), &NSString::from_str(TRANSIENT_TYPE));
        let our_change = pb.changeCount();

        let source = event_source();
        if let Err(e) = post_key(source.as_deref(), KEY_V, CGEventFlags::MaskCommand) {
            restore(&pb, saved);
            return Err(e);
        }
        // VERIFY ON MAC: keycode 9 is the physical 'V' position. On layouts
        // like "Dvorak" (not "Dvorak - QWERTY ⌘") ⌘+keycode 9 may not be Paste;
        // a fix is to look up the keycode for 'v' via UCKeyTranslate.

        let delay = self.restore_after;
        let handle = std::thread::Builder::new()
            .name("zwisper-clipboard-restore".into())
            .spawn(move || {
                std::thread::sleep(delay);
                // VERIFY ON MAC: NSPasteboard from a non-main thread. It is
                // documented as thread-safe for reading/writing in practice.
                let pb = NSPasteboard::generalPasteboard();
                if pb.changeCount() == our_change {
                    restore(&pb, saved);
                } else {
                    tracing::debug!("pasteboard changed since paste; not restoring");
                }
            })
            .map_err(|e| Error::InvalidState(format!("spawn restore thread: {e}")))?;
        *PENDING_RESTORE.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
        Ok(())
    }
}

/// Fallback: types the text with `CGEventKeyboardSetUnicodeString`, in chunks
/// of ≤ 20 UTF-16 units (the documented per-event limit), never splitting a
/// surrogate pair. Requires **Accessibility**. Callable from any thread.
pub struct TypingInjector {
    /// Pause between chunks so slow apps don't drop events.
    pub chunk_delay: Duration,
}

impl Default for TypingInjector {
    fn default() -> Self {
        TypingInjector { chunk_delay: Duration::from_millis(4) }
    }
}

impl TypingInjector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TextInjector for TypingInjector {
    fn id(&self) -> EngineId {
        "mac-typing"
    }

    fn inject(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if !accessibility_trusted() {
            return Err(Error::Permission(PermissionKind::Accessibility));
        }
        let source = event_source();
        for chunk in utf16_chunks(text, 20) {
            for down in [true, false] {
                // Keycode 0 is ignored once a unicode string is attached.
                let ev = CGEvent::new_keyboard_event(source.as_deref(), 0, down)
                    .ok_or_else(|| Error::InvalidState("CGEventCreateKeyboardEvent failed".into()))?;
                // No modifiers, so a still-held ⌥ doesn't turn letters into symbols.
                CGEvent::set_flags(Some(&ev), CGEventFlags::empty());
                // SAFETY: pointer/length describe the live `chunk` buffer.
                unsafe { CGEvent::keyboard_set_unicode_string(Some(&ev), chunk.len() as _, chunk.as_ptr()) };
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
            }
            std::thread::sleep(self.chunk_delay);
        }
        // VERIFY ON MAC: some apps (terminals, Electron) may treat '\n' in a
        // unicode-string event differently from Return.
        Ok(())
    }
}
