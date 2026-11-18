//! macOS OS-integration strategies.
//!
//! | Item | Port / purpose | Thread |
//! |---|---|---|
//! | [`MacHotkeys`] | `HotkeyService` (global-hotkey / Carbon) | `new()` on the **main thread** |
//! | [`PasteInjector`] | `TextInjector`: pasteboard + ⌘V, restores clipboard | any |
//! | [`TypingInjector`] | `TextInjector`: CGEvent unicode typing (fallback) | any |
//! | [`MacPermissions`] | `Permissions` | any |
//! | [`SystemClock`] | `Clock` | any |
//! | [`set_launch_at_login`] | login item via `SMAppService` | any |
//! | [`make_window_non_activating_panel`], [`set_activation_policy_accessory`] | UI helpers | main |
//! | [`system_memory_bytes`] | tier selection | any |
//!
//! On non-macOS targets only the platform-independent pieces compile
//! ([`chord`], [`typing`], [`SystemClock`], [`system_memory_bytes`]) so the
//! workspace builds on Linux CI. The composition root must gate the macOS
//! types behind `#[cfg(target_os = "macos")]`.

pub mod chord;
pub mod clock;
pub mod sysinfo;
pub mod typing;

pub use clock::SystemClock;
pub use sysinfo::system_memory_bytes;

#[cfg(target_os = "macos")]
mod hotkeys;
#[cfg(target_os = "macos")]
mod inject;
#[cfg(target_os = "macos")]
mod login;
#[cfg(target_os = "macos")]
mod permissions;
#[cfg(target_os = "macos")]
mod window;

#[cfg(target_os = "macos")]
pub use hotkeys::MacHotkeys;
#[cfg(target_os = "macos")]
pub use inject::{PasteInjector, TypingInjector};
#[cfg(target_os = "macos")]
pub use login::{launch_at_login_enabled, open_login_items_settings, set_launch_at_login};
#[cfg(target_os = "macos")]
pub use permissions::{accessibility_trusted, mark_system_audio_granted, settings_url, MacPermissions};
#[cfg(target_os = "macos")]
pub use window::{
    activate_app, make_window_non_activating_panel, order_front_without_activating,
    set_activation_policy_accessory,
};

#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    use super::*;
    use zwisper_core::ports::{Clock, HotkeyService, Permissions, TextInjector};

    #[test]
    fn port_impls() {
        fn is<T: ?Sized>() {}
        is::<dyn TextInjector>();
        let _: Box<dyn TextInjector> = Box::new(PasteInjector::new());
        let _: Box<dyn TextInjector> = Box::new(TypingInjector::new());
        let _: Box<dyn Permissions> = Box::new(MacPermissions::new());
        let _: Box<dyn Clock> = Box::new(SystemClock);
        fn hk<T: HotkeyService>() {}
        hk::<MacHotkeys>();
    }
}
