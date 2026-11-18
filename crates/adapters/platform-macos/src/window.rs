//! Window/activation helpers for the UI layer (not core ports).
//!
//! The dictation pill must **never take focus**: if it became key, or
//! activated our app, ⌘V would land in the pill instead of the user's app.
//!
//! ## Strategy (most robust first)
//!
//! 1. **Create the window as an `NSPanel`.** GPUI's `WindowKind::PopUp`
//!    allocates its panel class (an `NSPanel` subclass) — VERIFY ON MAC for the
//!    pinned GPUI rev. Then this helper only has to add
//!    `NSWindowStyleMaskNonactivatingPanel`, `floatingPanel` and
//!    `becomesKeyOnlyIfNeeded`. Only an `NSPanel` honours the non-activating
//!    style bit (it sets the window-server tag that stops a click on the window
//!    from activating the app).
//! 2. **Plain `NSWindow` fallback.** We can't turn an existing `NSWindow`
//!    into an `NSPanel`: `object_setClass` to `NSPanel` would drop the
//!    toolkit's own subclass (GPUI stores its state in ivars of its window
//!    class and overrides methods). Instead we isa-swizzle the window to a
//!    runtime-created *subclass of its current class* (like KVO does) that
//!    overrides `canBecomeKeyWindow`/`canBecomeMainWindow` → `NO`, so the
//!    window can never steal key focus even when clicked or ordered front.
//!    That alone doesn't stop a click from *activating the app*; for that we
//!    additionally call the private `-[NSWindow _setPreventsActivation:YES]`
//!    if the window responds to it (VERIFY ON MAC: private API; harmless no-op
//!    if absent; drop it if App Store review matters).
//! 3. Either way: status-bar window level, joins all Spaces, shows over
//!    full-screen apps, ignored by ⌘-Tab / window cycling, not hidden when the
//!    app deactivates. Show it with [`order_front_without_activating`]
//!    (`orderFrontRegardless`), never `makeKeyAndOrderFront:` / app activation.
//!
//! Combine with `set_activation_policy_accessory(true)` (LSUIElement-style
//! menu-bar app) so the app normally isn't frontmost at all.
//!
//! All functions here must run on the **main thread**; they return
//! `Error::InvalidState` otherwise.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, NSObjectProtocol, Sel};
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSPanel, NSStatusWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::ffi::{c_void, CString};
use zwisper_core::{Error, Result};

const SUBCLASS_PREFIX: &str = "ZwisperNoKey_";

fn main_thread() -> Result<MainThreadMarker> {
    MainThreadMarker::new().ok_or_else(|| Error::InvalidState("must be called on the main thread".into()))
}

unsafe extern "C-unwind" fn return_no(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::NO
}

/// A cached subclass of `base` whose instances can't become key or main.
fn no_key_subclass(base: &'static AnyClass) -> Result<&'static AnyClass> {
    let base_name = base.name().to_string_lossy().into_owned();
    if base_name.starts_with(SUBCLASS_PREFIX) {
        return Ok(base); // already swizzled
    }
    let name = CString::new(format!("{SUBCLASS_PREFIX}{base_name}"))
        .map_err(|_| Error::InvalidState("bad class name".into()))?;
    if let Some(existing) = AnyClass::get(&name) {
        return Ok(existing);
    }
    let mut builder = ClassBuilder::new(&name, base)
        .ok_or_else(|| Error::InvalidState(format!("could not subclass {base_name}")))?;
    // SAFETY: the implementations match the `- (BOOL)selector` signatures.
    unsafe {
        builder.add_method(sel!(canBecomeKeyWindow), return_no as unsafe extern "C-unwind" fn(_, _) -> _);
        builder.add_method(sel!(canBecomeMainWindow), return_no as unsafe extern "C-unwind" fn(_, _) -> _);
    }
    Ok(builder.register())
}

/// Turn an existing window (e.g. GPUI's, via its raw window handle) into a
/// floating, non-activating, never-key overlay. See the module docs.
///
/// # Safety
/// `ns_window` must be a valid, live `NSWindow*` (not an `NSView*`).
pub unsafe fn make_window_non_activating_panel(ns_window: *mut c_void) -> Result<()> {
    let _mtm = main_thread()?;
    let window: &NSWindow = (ns_window as *const NSWindow)
        .as_ref()
        .ok_or_else(|| Error::InvalidState("null NSWindow".into()))?;

    if let Some(panel) = window.downcast_ref::<NSPanel>() {
        // VERIFY ON MAC: toggling NonactivatingPanel after the panel was
        // created should update the window-server tag; if clicks still
        // activate the app, create the panel with the bit set instead.
        panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);
    } else {
        let current = window.class();
        let sub = no_key_subclass(current)?;
        if !std::ptr::eq(sub, current) {
            // SAFETY: `sub` is a subclass of the object's current class that
            // adds no ivars, so the instance layout is unchanged.
            // VERIFY ON MAC: don't do this while the window is being KVO-observed
            // with a dynamic NSKVONotifying_ class (GPUI doesn't, AFAIK).
            AnyObject::set_class(window, sub);
        }
        let private_sel = sel!(_setPreventsActivation:);
        if window.respondsToSelector(private_sel) {
            let _: () = msg_send![window, _setPreventsActivation: true];
        } else {
            tracing::debug!("_setPreventsActivation: unavailable; clicks on the pill may activate the app");
        }
    }

    window.setLevel(NSStatusWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::Stationary,
    );
    window.setHidesOnDeactivate(false);
    window.setCanHide(false);
    window.setExcludedFromWindowsMenu(true);
    Ok(())
}

/// Show a window without making it key and without activating the app.
///
/// # Safety
/// `ns_window` must be a valid, live `NSWindow*`.
pub unsafe fn order_front_without_activating(ns_window: *mut c_void) -> Result<()> {
    let _mtm = main_thread()?;
    let window: &NSWindow = (ns_window as *const NSWindow)
        .as_ref()
        .ok_or_else(|| Error::InvalidState("null NSWindow".into()))?;
    window.orderFrontRegardless();
    Ok(())
}

/// `true` → `.accessory` (menu-bar app: no Dock icon, not in ⌘-Tab);
/// `false` → `.regular` (while the Library / meeting window is open).
/// Switching to `.regular` doesn't activate the app; call
/// `activate_app()` afterwards if the window should come to the front.
pub fn set_activation_policy_accessory(accessory: bool) -> Result<()> {
    let mtm = main_thread()?;
    let app: Retained<NSApplication> = NSApplication::sharedApplication(mtm);
    let policy = if accessory {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    };
    if !app.setActivationPolicy(policy) {
        return Err(Error::InvalidState("setActivationPolicy was refused".into()));
    }
    Ok(())
}

/// Bring the app to the front (e.g. after switching to `.regular`).
pub fn activate_app() -> Result<()> {
    let mtm = main_thread()?;
    let app = NSApplication::sharedApplication(mtm);
    // `activate` (macOS 14+) replaces the deprecated activateIgnoringOtherApps:.
    app.activate();
    Ok(())
}
