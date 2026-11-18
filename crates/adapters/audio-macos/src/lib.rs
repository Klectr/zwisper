//! macOS audio capture strategies.
//!
//! * [`MacAudio`] implements `AudioSourceFactory`:
//!   - `microphone()` → `CpalMic` (cpal / CoreAudio input stream)
//!   - `system_output()` → `CoreAudioTap` (Core Audio process tap, macOS 14.2+)
//!
//! All frames are delivered as 16 kHz mono f32. On non-macOS targets the crate
//! compiles to a stub factory (no microphones, no system audio) so the
//! workspace builds on Linux CI; only [`convert`] has real logic there.

pub mod convert;

#[cfg(target_os = "macos")]
mod mic;
#[cfg(target_os = "macos")]
mod tap;

#[cfg(target_os = "macos")]
pub use mic::CpalMic;
#[cfg(target_os = "macos")]
pub use tap::CoreAudioTap;

use zwisper_core::ports::{AudioSource, AudioSourceFactory};
#[cfg(not(target_os = "macos"))]
use zwisper_core::Error;
use zwisper_core::Result;

/// The macOS `AudioSourceFactory`. Cheap to construct; holds no OS resources.
/// Thread-safe: any method may be called from any thread.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacAudio;

impl MacAudio {
    pub fn new() -> Self {
        MacAudio
    }
}

#[cfg(target_os = "macos")]
impl AudioSourceFactory for MacAudio {
    fn microphone(&self, device: Option<&str>) -> Result<Box<dyn AudioSource>> {
        Ok(Box::new(mic::CpalMic::new(device)?))
    }

    fn system_output(&self) -> Result<Option<Box<dyn AudioSource>>> {
        if !tap::os_supports_taps() {
            tracing::info!("system audio capture needs macOS 14.2+");
            return Ok(None);
        }
        match tap::CoreAudioTap::new() {
            Ok(t) => Ok(Some(Box::new(t))),
            Err(tap::TapError::Permission) => Err(zwisper_core::Error::Permission(
                zwisper_core::domain::PermissionKind::SystemAudio,
            )),
            Err(tap::TapError::Unavailable(reason)) => {
                tracing::warn!(%reason, "system audio tap unavailable");
                Ok(None)
            }
        }
    }

    fn list_microphones(&self) -> Vec<String> {
        mic::list_input_names()
    }
}

/// Non-macOS stub: keeps the workspace building on Linux CI.
#[cfg(not(target_os = "macos"))]
impl AudioSourceFactory for MacAudio {
    fn microphone(&self, _device: Option<&str>) -> Result<Box<dyn AudioSource>> {
        Err(Error::Audio("zwisper-audio-macos only captures audio on macOS".into()))
    }

    fn system_output(&self) -> Result<Option<Box<dyn AudioSource>>> {
        Ok(None)
    }

    fn list_microphones(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_factory<T: AudioSourceFactory + 'static>() {}

    #[test]
    fn is_a_factory() {
        assert_factory::<MacAudio>();
        let _: Box<dyn AudioSourceFactory> = Box::new(MacAudio::new());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_stub_has_no_system_audio() {
        assert!(MacAudio.system_output().unwrap().is_none());
        assert!(MacAudio.list_microphones().is_empty());
    }
}
