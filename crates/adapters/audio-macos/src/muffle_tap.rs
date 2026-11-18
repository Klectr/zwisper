//! Muffle every other app's sound while dictating.
//!
//! A global process tap (excluding us) with `CATapMutedWhenTapped`: while we
//! read it, macOS silences those apps on the real device. A private aggregate
//! device combines that tap (input) with the default output device (output);
//! its IOProc low-passes the tapped audio ([`Muffler`]) and plays it out.
//! Net effect: you still hear the room, not the words.
//!
//! Safety net: the tap belongs to our process, so if we crash or exit the
//! mute ends with us and the original sound comes straight back.
//!
//! Needs the System Audio Recording permission; callers check it first so the
//! prompt never appears mid-dictation.

use crate::muffle::Muffler;
use crate::tap::{check, dict, fourcc, get_prop, obj, own_process_object};
use crate::volume::default_output_device;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat, AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID,
    AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectID, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

type OSStatus = i32;

/// Largest IO buffer we handle without allocating on the audio thread.
const MAX_FRAMES: usize = 16_384;

struct State {
    muffler: Muffler,
    /// Target amount (f32 bits), set from the ducker thread.
    target: Arc<AtomicU32>,
    left: Vec<f32>,
    right: Vec<f32>,
}

/// # Safety
/// `list` must be a valid AudioBufferList.
unsafe fn buffers<'a>(list: *const AudioBufferList) -> &'a [AudioBuffer] {
    let n = (*list).mNumberBuffers as usize;
    std::slice::from_raw_parts(std::ptr::addr_of!((*list).mBuffers) as *const AudioBuffer, n)
}

/// # Safety
/// `b.mData` must be null or valid for `mDataByteSize` bytes of f32.
unsafe fn samples<'a>(b: &AudioBuffer) -> &'a mut [f32] {
    if b.mData.is_null() {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(b.mData as *mut f32, b.mDataByteSize as usize / 4)
    }
}

impl State {
    /// # Safety
    /// Valid buffer lists from the HAL; the tap delivers 32-bit float.
    unsafe fn render(&mut self, input: *const AudioBufferList, output: *mut AudioBufferList) {
        let ins = buffers(input);
        // The tap's streams come after any input streams of the output device,
        // so take the last buffers that add up to its two channels.
        // VERIFY ON MAC: with a headset (output device that also has inputs),
        // check the muffled sound is the apps' audio, not the headset mic.
        let mut idx = ins.len();
        let mut need = 2usize;
        while need > 0 && idx > 0 {
            idx -= 1;
            need = need.saturating_sub(ins[idx].mNumberChannels.max(1) as usize);
        }
        let tap = &ins[idx..];

        let frames = match tap {
            [] => 0,
            [only] => {
                let ch = only.mNumberChannels.max(1) as usize;
                let s = samples(only);
                let n = (s.len() / ch).min(MAX_FRAMES);
                for i in 0..n {
                    self.left[i] = s[i * ch];
                    self.right[i] = if ch > 1 { s[i * ch + 1] } else { s[i * ch] };
                }
                n
            }
            [l, r, ..] => {
                let (l, r) = (samples(l), samples(r));
                let n = l.len().min(r.len()).min(MAX_FRAMES);
                self.left[..n].copy_from_slice(&l[..n]);
                self.right[..n].copy_from_slice(&r[..n]);
                n
            }
        };

        let target = f32::from_bits(self.target.load(Ordering::Relaxed));
        let (l, r) = (&mut self.left[..frames], &mut self.right[..frames]);
        self.muffler.process(l, r, target);

        // VERIFY ON MAC: output buffers are the device's virtual format, which
        // is 32-bit float on every device seen so far.
        let outs = buffers(output);
        let total_out: usize = outs.iter().map(|b| b.mNumberChannels.max(1) as usize).sum();
        let mut first_channel = 0usize;
        for b in outs {
            let ch = b.mNumberChannels.max(1) as usize;
            let out = samples(b);
            for (i, frame) in out.chunks_mut(ch).enumerate() {
                for (c, v) in frame.iter_mut().enumerate() {
                    *v = if i >= frames {
                        0.0
                    } else if total_out == 1 {
                        0.5 * (self.left[i] + self.right[i])
                    } else {
                        match first_channel + c {
                            0 => self.left[i],
                            1 => self.right[i],
                            _ => 0.0,
                        }
                    };
                }
            }
            first_channel += ch;
        }
    }
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> OSStatus {
    if client.is_null() {
        return 0;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `client` is the boxed State registered in `start`, freed only
        // after the IOProc is destroyed; the HAL never re-enters one IOProc.
        let state = &mut *(client as *mut State);
        state.render(input.as_ptr(), output.as_ptr());
    }));
    0
}

/// A running muffle. Dropping it stops the IOProc and removes the tap, which
/// un-mutes the other apps.
pub(crate) struct MuffleTap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    state: *mut State,
    target: Arc<AtomicU32>,
}

// SAFETY: the raw state pointer is only used by the IOProc while registered
// and freed in Drop after the IOProc is destroyed; the HAL calls are thread-safe.
unsafe impl Send for MuffleTap {}

fn device_uid(dev: AudioObjectID) -> Option<Retained<NSString>> {
    // SAFETY: DeviceUID is a +1 CFStringRef, toll-free bridged to NSString.
    unsafe {
        let raw = get_prop::<*mut c_void>(dev, kAudioDevicePropertyDeviceUID, None, std::ptr::null_mut()).ok()?;
        Retained::from_raw(raw as *mut NSString)
    }
}

impl MuffleTap {
    /// Start the tap with the effect open (target 0); call [`set_target`]
    /// to glide towards muffled.
    ///
    /// [`set_target`]: MuffleTap::set_target
    pub(crate) fn start() -> Result<MuffleTap, String> {
        let mut this =
            MuffleTap { tap: 0, aggregate: 0, proc_id: None, state: std::ptr::null_mut(), target: Arc::new(AtomicU32::new(0)) };

        let exclude: Vec<Retained<NSNumber>> = own_process_object().map(NSNumber::new_u32).into_iter().collect();
        let exclude = NSArray::from_retained_slice(&exclude);
        // SAFETY: plain Objective-C init / setters on a fresh object.
        let desc = unsafe {
            let d = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &exclude);
            d.setName(&NSString::from_str("Zwisper muffle"));
            d.setPrivate(true);
            d.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
            d
        };
        // SAFETY: readonly property.
        let tap_uuid = unsafe { desc.UUID() }.UUIDString();
        // SAFETY: valid description and out-pointer.
        check(unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut this.tap) }, "create muffle tap")
            .map_err(|e| format!("{e:?}"))?;

        let zero = AudioStreamBasicDescription {
            mSampleRate: 0.0,
            mFormatID: 0,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 0,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        // SAFETY: kAudioTapPropertyFormat is an ASBD.
        let asbd = unsafe { get_prop(this.tap, kAudioTapPropertyFormat, None, zero) }
            .map_err(|s| format!("read tap format: {}", fourcc(s)))?;
        let is_f32 = asbd.mFormatFlags & crate::convert::FLAG_IS_FLOAT != 0 && asbd.mBitsPerChannel == 32;
        if !is_f32 {
            return Err(format!("tap format isn't 32-bit float: {asbd:?}"));
        }

        let out_dev = default_output_device().ok_or("no default output device")?;
        let out_uid = device_uid(out_dev).ok_or("no output device UID")?;
        let tap_entry = dict(vec![
            (kAudioSubTapUIDKey, obj(tap_uuid)),
            (kAudioSubTapDriftCompensationKey, obj(NSNumber::new_bool(true))),
        ]);
        let sub = dict(vec![(kAudioSubDeviceUIDKey, obj(out_uid.clone()))]);
        let description = dict(vec![
            (kAudioAggregateDeviceNameKey, obj(NSString::from_str("Zwisper Muffle"))),
            (kAudioAggregateDeviceUIDKey, obj(NSUUID::new().UUIDString())),
            (kAudioAggregateDeviceIsPrivateKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceIsStackedKey, obj(NSNumber::new_bool(false))),
            (kAudioAggregateDeviceTapAutoStartKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceMainSubDeviceKey, obj(out_uid)),
            (kAudioAggregateDeviceSubDeviceListKey, obj(NSArray::from_retained_slice(&[sub]))),
            (kAudioAggregateDeviceTapListKey, obj(NSArray::from_retained_slice(&[tap_entry]))),
        ]);
        // SAFETY: NSDictionary is toll-free bridged with CFDictionary.
        let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
        check(
            unsafe { AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut this.aggregate)) },
            "create muffle aggregate",
        )
        .map_err(|e| format!("{e:?}"))?;

        // SAFETY: NominalSampleRate is a Float64.
        let rate = unsafe { get_prop::<f64>(this.aggregate, kAudioDevicePropertyNominalSampleRate, None, 0.0) }
            .unwrap_or(asbd.mSampleRate);
        tracing::info!(rate, tap_rate = asbd.mSampleRate, "starting muffle");

        this.state = Box::into_raw(Box::new(State {
            muffler: Muffler::new(rate as f32),
            target: this.target.clone(),
            left: vec![0.0; MAX_FRAMES],
            right: vec![0.0; MAX_FRAMES],
        }));
        // SAFETY: io_proc matches AudioDeviceIOProc; state outlives the IOProc (see Drop).
        check(
            unsafe {
                AudioDeviceCreateIOProcID(this.aggregate, Some(io_proc), this.state as *mut c_void, NonNull::from(&mut this.proc_id))
            },
            "create muffle IOProc",
        )
        .map_err(|e| format!("{e:?}"))?;
        // SAFETY: valid device + proc id.
        check(unsafe { AudioDeviceStart(this.aggregate, this.proc_id) }, "start muffle").map_err(|e| format!("{e:?}"))?;
        Ok(this)
    }

    /// 0 = open (untouched) … 1 = fully muffled; the IOProc glides there.
    pub(crate) fn set_target(&self, amount: f32) {
        self.target.store(amount.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

impl Drop for MuffleTap {
    fn drop(&mut self) {
        // SAFETY: ids were created by us; each is destroyed exactly once, in
        // reverse order, and the state is freed only after its IOProc is gone.
        unsafe {
            if self.proc_id.is_some() {
                let _ = AudioDeviceStop(self.aggregate, self.proc_id);
                let _ = AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            }
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
            if self.aggregate != 0 {
                let s = AudioHardwareDestroyAggregateDevice(self.aggregate);
                if s != 0 {
                    tracing::warn!(status = %fourcc(s), "destroy muffle aggregate failed");
                }
            }
            if self.tap != 0 {
                let s = AudioHardwareDestroyProcessTap(self.tap);
                if s != 0 {
                    tracing::warn!(status = %fourcc(s), "destroy muffle tap failed");
                }
            }
        }
    }
}
