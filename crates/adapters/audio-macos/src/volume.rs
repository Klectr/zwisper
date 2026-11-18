//! Lowering the default output device's volume, and putting it back.
//!
//! What we changed is written to a small state file first, so if the app dies
//! mid-dictation the next launch restores the volume (see [`recover`]).

use crate::tap::get_prop;
use objc2_core_audio::{
    kAudioDevicePropertyVolumeScalar, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectIsPropertySettable, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData,
};
use std::path::Path;
use std::ptr::NonNull;

/// If the volume now differs from what we set by more than this, the user
/// changed it themselves and we leave it alone.
const USER_CHANGED_TOLERANCE: f32 = 0.02;

/// The device media plays through (not the alert-sound device).
pub(crate) fn default_output_device() -> Option<AudioObjectID> {
    // SAFETY: DefaultOutputDevice is an AudioObjectID.
    let dev = unsafe {
        get_prop::<AudioObjectID>(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultOutputDevice, None, 0)
    }
    .ok()?;
    (dev != 0).then_some(dev)
}

fn volume_addr(element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyVolumeScalar,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: element,
    }
}

fn settable(dev: AudioObjectID, element: u32) -> bool {
    let mut addr = volume_addr(element);
    let mut yes: u8 = 0;
    // SAFETY: valid address and out-pointer (Boolean is a u8).
    let status = unsafe { AudioObjectIsPropertySettable(dev, NonNull::from(&mut addr), NonNull::from(&mut yes).cast()) };
    status == 0 && yes != 0
}

fn get_volume(dev: AudioObjectID, element: u32) -> Option<f32> {
    let mut addr = volume_addr(element);
    let mut value: f32 = 0.0;
    let mut size = std::mem::size_of::<f32>() as u32;
    // SAFETY: VolumeScalar is a Float32.
    let status = unsafe {
        AudioObjectGetPropertyData(
            dev,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0).then_some(value)
}

fn set_volume(dev: AudioObjectID, element: u32, value: f32) -> bool {
    let mut addr = volume_addr(element);
    let mut value = value.clamp(0.0, 1.0);
    // SAFETY: VolumeScalar is a Float32.
    let status = unsafe {
        AudioObjectSetPropertyData(
            dev,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            std::mem::size_of::<f32>() as u32,
            NonNull::from(&mut value).cast(),
        )
    };
    status == 0
}

/// A volume change we made: per element, (original, what we set).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Lowered {
    device: AudioObjectID,
    elements: Vec<(u32, f32, f32)>,
}

impl Lowered {
    /// Scale the default output's volume by `factor`. `None` if the device
    /// has no settable volume (HDMI, many USB interfaces).
    pub(crate) fn apply(factor: f32) -> Option<Lowered> {
        let device = default_output_device()?;
        // Prefer the main (virtual) control; else the stereo channels.
        let elements: Vec<u32> = if settable(device, kAudioObjectPropertyElementMain) {
            vec![kAudioObjectPropertyElementMain]
        } else {
            [1, 2].into_iter().filter(|&e| settable(device, e)).collect()
        };
        let mut changed = Vec::new();
        for e in elements {
            if let Some(orig) = get_volume(device, e) {
                let ours = orig * factor;
                if set_volume(device, e, ours) {
                    // Store what the device actually reports (it may quantize).
                    changed.push((e, orig, get_volume(device, e).unwrap_or(ours)));
                }
            }
        }
        if changed.is_empty() {
            tracing::info!(device, "output device has no settable volume; not lowering");
            return None;
        }
        Some(Lowered { device, elements: changed })
    }

    /// Put the original volume back, unless the user has since changed it.
    pub(crate) fn restore(&self) {
        for &(e, orig, ours) in &self.elements {
            match get_volume(self.device, e) {
                Some(now) if (now - ours).abs() <= USER_CHANGED_TOLERANCE => {
                    set_volume(self.device, e, orig);
                }
                Some(_) => tracing::debug!(element = e, "volume changed by the user; leaving it"),
                None => {} // device gone
            }
        }
    }

    fn serialize(&self) -> String {
        let mut s = self.device.to_string();
        for (e, orig, ours) in &self.elements {
            s.push_str(&format!(" {e}:{orig}:{ours}"));
        }
        s
    }

    fn parse(s: &str) -> Option<Lowered> {
        let mut parts = s.split_whitespace();
        let device = parts.next()?.parse().ok()?;
        let elements = parts
            .map(|p| {
                let mut f = p.split(':');
                Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Lowered { device, elements })
    }

    pub(crate) fn save(&self, path: &Path) {
        if let Err(e) = std::fs::write(path, self.serialize()) {
            tracing::warn!(error = %e, "could not save ducking state");
        }
    }
}

/// After a crash mid-dictation: restore the volume we lowered, if any.
pub(crate) fn recover(path: &Path) {
    if let Ok(s) = std::fs::read_to_string(path) {
        if let Some(l) = Lowered::parse(&s) {
            tracing::info!("restoring output volume left lowered by a previous run");
            l.restore();
        }
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let l = Lowered { device: 73, elements: vec![(0, 0.8, 0.24), (2, 0.5, 0.15)] };
        assert_eq!(Lowered::parse(&l.serialize()), Some(l));
        assert_eq!(Lowered::parse("garbage"), None);
    }

    /// Touches the real default output device (silently): lowers, checks,
    /// restores. `cargo test -p zwisper-audio-macos -- --ignored volume`
    #[test]
    #[ignore]
    fn lowers_and_restores_the_real_output_volume() {
        let Some(l) = Lowered::apply(0.3) else {
            eprintln!("default output has no settable volume; nothing to check");
            return;
        };
        for &(e, orig, ours) in &l.elements {
            let now = get_volume(l.device, e).unwrap();
            assert!((now - ours).abs() < 0.02 && ours <= orig, "element {e}: orig {orig}, set {ours}, now {now}");
        }
        l.restore();
        for &(e, orig, _) in &l.elements {
            let now = get_volume(l.device, e).unwrap();
            assert!((now - orig).abs() < 0.02, "element {e}: expected {orig} back, got {now}");
        }
    }
}
