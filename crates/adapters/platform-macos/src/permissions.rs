//! `Permissions` for macOS: Microphone (AVFoundation), Accessibility (AX) and
//! System Audio Recording (no public status API).

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
            PermissionKind::SystemAudio => {
                if SYSTEM_AUDIO_GRANTED.load(Ordering::Relaxed) {
                    PermissionState::Granted
                } else {
                    PermissionState::NotDetermined
                }
            }
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
            // There is no API to prompt for System Audio Recording; the prompt
            // appears the first time a process tap starts delivering audio.
            PermissionKind::SystemAudio => {}
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
