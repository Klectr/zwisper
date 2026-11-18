//! `Permissions` for macOS: Microphone (AVFoundation), Accessibility (AX) and
//! System Audio Recording (private TCC SPI, see `tcc`).

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_application_services::{kAXTrustedCheckOptionPrompt, AXIsProcessTrusted, AXIsProcessTrustedWithOptions};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::sync::atomic::{AtomicBool, Ordering};
use zwisper_core::domain::{PermissionKind, PermissionState};
use zwisper_core::ports::Permissions;

static SYSTEM_AUDIO_GRANTED: AtomicBool = AtomicBool::new(false);

/// Record that system-audio capture demonstrably works (e.g. the tap delivered
/// non-silent audio). macOS has no public API to query the "System Audio
/// Recording Only" permission, so `MacPermissions` reports `NotDetermined`
/// for it until the app calls this.
pub fn mark_system_audio_granted() {
    SYSTEM_AUDIO_GRANTED.store(true, Ordering::Relaxed);
}

/// System Audio Recording via the private TCC framework — the only way to
/// prompt for, or read, `kTCCServiceAudioCapture` before a tap runs. Fine for a
/// Developer ID build (not App Store). Loaded with `dlopen` so a future macOS
/// that drops the symbols degrades to "open System Settings" instead of crashing.
mod tcc {
    use block2::RcBlock;
    use objc2_core_foundation::{CFDictionary, CFString};
    use std::ffi::{c_int, c_void, CStr};
    use std::sync::OnceLock;

    type PreflightFn = unsafe extern "C" fn(service: *const CFString, options: *const CFDictionary) -> c_int;
    type RequestFn = unsafe extern "C" fn(
        service: *const CFString,
        options: *const CFDictionary,
        callback: *const block2::Block<dyn Fn(u8)>,
    );

    struct Api {
        preflight: PreflightFn,
        request: RequestFn,
    }

    const SERVICE: &str = "kTCCServiceAudioCapture";

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            let path = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
            // SAFETY: dlopen/dlsym with valid NUL-terminated strings; the handle is
            // intentionally never closed so the function pointers stay valid.
            unsafe {
                let h = libc::dlopen(path.as_ptr(), libc::RTLD_NOW);
                if h.is_null() {
                    tracing::warn!("TCC.framework unavailable");
                    return None;
                }
                let sym = |name: &CStr| -> *mut c_void { libc::dlsym(h, name.as_ptr()) };
                let (p, r) = (sym(c"TCCAccessPreflight"), sym(c"TCCAccessRequest"));
                if p.is_null() || r.is_null() {
                    tracing::warn!("TCC preflight/request symbols missing");
                    return None;
                }
                Some(Api {
                    preflight: std::mem::transmute::<*mut c_void, PreflightFn>(p),
                    request: std::mem::transmute::<*mut c_void, RequestFn>(r),
                })
            }
        })
        .as_ref()
    }

    /// `Some(true)` granted, `Some(false)` denied, `None` not asked / unknown.
    pub fn preflight() -> Option<bool> {
        let api = api()?;
        let service = CFString::from_str(SERVICE);
        // SAFETY: valid CFString, null options.
        match unsafe { (api.preflight)(&*service, std::ptr::null()) } {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }

    /// Shows the system prompt. Returns false if the SPI is unavailable.
    pub fn request(on_answer: impl Fn(bool) + 'static) -> bool {
        let Some(api) = api() else { return false };
        let service = CFString::from_str(SERVICE);
        let block = RcBlock::new(move |granted: u8| on_answer(granted != 0));
        // SAFETY: valid CFString, null options; TCC copies the block.
        unsafe { (api.request)(&*service, std::ptr::null(), &*block) };
        true
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn tcc_spi_resolves_on_this_macos() {
            assert!(super::api().is_some(), "TCCAccessPreflight/Request not found");
            // Any answer is fine; this just must not crash.
            let _ = super::preflight();
        }
    }
}

/// `AXIsProcessTrusted()`: may this process post synthetic input events?
pub fn accessibility_trusted() -> bool {
    // SAFETY: no arguments; pure query.
    unsafe { AXIsProcessTrusted() }
}

/// System Settings deep links (Privacy & Security panes).
pub fn settings_url(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        PermissionKind::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        // VERIFY ON MAC: anchor for "Screen & System Audio Recording" on 14.4+/15.
        PermissionKind::SystemAudio => "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture",
    }
}

/// Stateless; every method is safe to call from any thread.
/// `request` never blocks; the OS prompt (if any) appears asynchronously.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacPermissions;

impl MacPermissions {
    pub fn new() -> Self {
        MacPermissions
    }
}

fn audio_media_type() -> Option<&'static objc2_av_foundation::AVMediaType> {
    // SAFETY: framework-provided immutable constant.
    unsafe { AVMediaTypeAudio }
}

impl Permissions for MacPermissions {
    fn state(&self, kind: PermissionKind) -> PermissionState {
        match kind {
            PermissionKind::Microphone => {
                let Some(mt) = audio_media_type() else { return PermissionState::NotDetermined };
                // SAFETY: valid media type constant.
                let s = unsafe { AVCaptureDevice::authorizationStatusForMediaType(mt) };
                match s {
                    AVAuthorizationStatus::Authorized => PermissionState::Granted,
                    AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => PermissionState::Denied,
                    _ => PermissionState::NotDetermined,
                }
            }
            // AX has no "denied" state: untrusted means "not granted yet".
            PermissionKind::Accessibility => {
                if accessibility_trusted() {
                    PermissionState::Granted
                } else {
                    PermissionState::NotDetermined
                }
            }
            PermissionKind::SystemAudio => match tcc::preflight() {
                Some(true) => PermissionState::Granted,
                Some(false) => PermissionState::Denied,
                None if SYSTEM_AUDIO_GRANTED.load(Ordering::Relaxed) => PermissionState::Granted,
                None => PermissionState::NotDetermined,
            },
        }
    }

    fn request(&self, kind: PermissionKind) {
        match kind {
            PermissionKind::Microphone => {
                let Some(mt) = audio_media_type() else { return };
                let handler = RcBlock::new(|granted: Bool| {
                    tracing::info!(granted = granted.as_bool(), "microphone permission answered");
                });
                // SAFETY: AVFoundation copies the block; handler is called on an arbitrary queue.
                unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(mt, &handler) };
            }
            PermissionKind::Accessibility => {
                // SAFETY: framework constant; CFString is toll-free bridged with NSString.
                let key: &NSString = unsafe { &*(kAXTrustedCheckOptionPrompt as *const _ as *const NSString) };
                let value = NSNumber::new_bool(true);
                let dict: Retained<NSDictionary<NSString, NSNumber>> = NSDictionary::from_slices(&[key], &[&*value]);
                // SAFETY: NSDictionary is toll-free bridged with CFDictionary.
                let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&dict) as *const CFDictionary) };
                // Shows the "would like to control this computer" alert if untrusted.
                let _ = unsafe { AXIsProcessTrustedWithOptions(Some(cf)) };
            }
            // No public API prompts for System Audio Recording; use the TCC SPI,
            // and if that's gone, send the user to the Settings pane instead.
            PermissionKind::SystemAudio => {
                let asked = tcc::request(|granted| {
                    tracing::info!(granted, "system audio permission answered");
                    if granted {
                        mark_system_audio_granted();
                    }
                });
                if !asked {
                    self.open_settings(kind);
                }
            }
        }
    }

    fn open_settings(&self, kind: PermissionKind) {
        let url = settings_url(kind);
        let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
            tracing::warn!(url, "invalid settings URL");
            return;
        };
        // VERIFY ON MAC: NSWorkspace openURL off the main thread (documented thread-safe).
        if !NSWorkspace::sharedWorkspace().openURL(&nsurl) {
            tracing::warn!(url, "could not open System Settings");
        }
    }
}
