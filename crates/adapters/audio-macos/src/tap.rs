//! System-output capture through a Core Audio process tap (macOS 14.2+).
//!
//! Sequence (after Apple's "Capturing system audio with Core Audio taps"
//! sample and insidegui/AudioCap):
//! 1. `CATapDescription initStereoGlobalTapButExcludeProcesses:` (excluding
//!    our own process so pill sounds etc. aren't recorded), private, unmuted.
//! 2. `AudioHardwareCreateProcessTap` → tap object id.
//! 3. Read the tap's stream format (`kAudioTapPropertyFormat`).
//! 4. `AudioHardwareCreateAggregateDevice` with a private aggregate whose tap
//!    list contains the tap UUID (and the default output device as clock).
//! 5. `AudioDeviceCreateIOProcID` + `AudioDeviceStart` on the aggregate.
//!
//! Teardown is the reverse: stop → destroy IOProc → destroy aggregate → destroy tap.

use crate::convert::{decode_into, mix_planar, PcmFormat, SampleKind};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePermissionsError,
    kAudioDevicePropertyDeviceUID, kAudioHardwarePropertyDefaultSystemOutputDevice,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioSubDeviceUIDKey,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData,
    AudioObjectID, AudioObjectPropertyAddress, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSObject, NSOperatingSystemVersion, NSProcessInfo, NSString,
    NSUUID,
};
use std::ffi::{c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use zwisper_core::domain::{PermissionKind, TrackKind};
use zwisper_core::dsp::{downmix, Resampler};
use zwisper_core::ports::{AudioSource, FrameSink};
use zwisper_core::{Error, Result};

type OSStatus = i32;

/// Why tap creation failed, so the factory can map it to `Ok(None)` or `Err`.
#[derive(Debug)]
pub(crate) enum TapError {
    Permission,
    Unavailable(String),
}

fn fourcc(s: OSStatus) -> String {
    let b = s.to_be_bytes();
    if b.iter().all(|c| c.is_ascii_graphic()) {
        format!("'{}' ({s})", String::from_utf8_lossy(&b))
    } else {
        s.to_string()
    }
}

fn check(status: OSStatus, what: &str) -> std::result::Result<(), TapError> {
    if status == 0 {
        Ok(())
    } else if status == kAudioDevicePermissionsError {
        // VERIFY ON MAC: which OSStatus a TCC denial actually produces. In
        // practice macOS may let creation succeed and deliver silence instead.
        Err(TapError::Permission)
    } else {
        Err(TapError::Unavailable(format!("{what} failed: OSStatus {}", fourcc(status))))
    }
}

/// VERIFY ON MAC: the System Audio Recording prompt appears on first
/// `AudioDeviceStart`; if the user denies it, capture likely "succeeds" but
/// delivers silence. The app may want to detect all-zero audio for a few
/// seconds and point the user at `Permissions::open_settings(SystemAudio)`.
pub(crate) fn os_supports_taps() -> bool {
    let v = NSOperatingSystemVersion { majorVersion: 14, minorVersion: 2, patchVersion: 0 };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(v)
}

fn global_addr(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size POD property.
///
/// # Safety
/// `T` must be the exact type CoreAudio stores for `selector` on `object`.
unsafe fn get_prop<T: Copy>(
    object: AudioObjectID,
    selector: u32,
    qualifier: Option<&[u8]>,
    zero: T,
) -> std::result::Result<T, OSStatus> {
    let mut addr = global_addr(selector);
    let mut value = zero;
    let mut size = std::mem::size_of::<T>() as u32;
    let (q_len, q_ptr) = match qualifier {
        Some(q) => (q.len() as u32, q.as_ptr() as *const c_void),
        None => (0, std::ptr::null()),
    };
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&mut addr),
        q_len,
        q_ptr,
        NonNull::from(&mut size),
        NonNull::from(&mut value).cast(),
    );
    if status == 0 {
        Ok(value)
    } else {
        Err(status)
    }
}

/// Our own process as a Core Audio process object (to exclude from the tap).
fn own_process_object() -> Option<AudioObjectID> {
    let pid: i32 = NSProcessInfo::processInfo().processIdentifier();
    // SAFETY: TranslatePIDToProcessObject takes a pid_t qualifier and returns an AudioObjectID.
    let obj = unsafe {
        get_prop::<AudioObjectID>(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            Some(&pid.to_ne_bytes()),
            0,
        )
    }
    .ok()?;
    // 0 == kAudioObjectUnknown: we haven't produced audio yet, so there is nothing to exclude.
    (obj != 0).then_some(obj)
}

/// UID of the default system output device, used as the aggregate's clock source.
fn default_output_uid() -> Option<Retained<NSString>> {
    // SAFETY: DefaultSystemOutputDevice is an AudioObjectID.
    let dev = unsafe {
        get_prop::<AudioObjectID>(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyDefaultSystemOutputDevice,
            None,
            0,
        )
    }
    .ok()?;
    // SAFETY: DeviceUID is a CFStringRef returned at +1 (caller releases), and
    // CFString is toll-free bridged with NSString, so `Retained::from_raw`
    // takes ownership of that reference.
    unsafe {
        let raw = get_prop::<*mut c_void>(dev, kAudioDevicePropertyDeviceUID, None, std::ptr::null_mut()).ok()?;
        Retained::from_raw(raw as *mut NSString)
    }
}

fn key(k: &CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().expect("CoreAudio keys are ASCII"))
}

/// Upcast a Foundation object to `NSObject` for use as a dictionary value.
fn obj<T: objc2::Message>(r: Retained<T>) -> Retained<NSObject> {
    // SAFETY: only called with Foundation classes (NSString/NSNumber/NSArray/
    // NSDictionary), all of which inherit from NSObject.
    unsafe { Retained::cast_unchecked(r) }
}

fn dict(entries: Vec<(&CStr, Retained<NSObject>)>) -> Retained<NSDictionary<NSString, NSObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| key(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<Retained<NSObject>> = entries.into_iter().map(|(_, v)| v).collect();
    NSDictionary::from_retained_objects(&key_refs, &values)
}

/// Created Core Audio objects. `Drop` tears them down in the right order.
struct TapObjects {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    format: PcmFormat,
}

impl TapObjects {
    fn create() -> std::result::Result<Self, TapError> {
        let exclude: Vec<Retained<NSNumber>> =
            own_process_object().map(NSNumber::new_u32).into_iter().collect();
        let exclude = NSArray::from_retained_slice(&exclude);

        // SAFETY: plain Objective-C init / property setters on a fresh object.
        let desc = unsafe {
            let d = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &exclude);
            d.setName(&NSString::from_str("Zwisper system audio"));
            d.setPrivate(true);
            d.setMuteBehavior(CATapMuteBehavior::Unmuted);
            d
        };
        // SAFETY: UUID is a readonly property of the description.
        let tap_uuid = unsafe { desc.UUID() }.UUIDString();

        let mut tap: AudioObjectID = 0;
        // SAFETY: valid description and out-pointer.
        check(unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut tap) }, "AudioHardwareCreateProcessTap")?;

        // From here on, clean up the tap if anything else fails.
        let mut this = TapObjects { tap, aggregate: 0, format: PcmFormat::default_float(48_000.0, 2) };

        let zero_asbd = AudioStreamBasicDescription {
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
        // SAFETY: kAudioTapPropertyFormat is an AudioStreamBasicDescription.
        let asbd = unsafe { get_prop(tap, kAudioTapPropertyFormat, None, zero_asbd) }
            .map_err(|s| TapError::Unavailable(format!("read tap format: OSStatus {}", fourcc(s))))?;
        this.format = PcmFormat {
            sample_rate: asbd.mSampleRate,
            format_id: asbd.mFormatID,
            flags: asbd.mFormatFlags,
            bits_per_channel: asbd.mBitsPerChannel,
            channels_per_frame: asbd.mChannelsPerFrame,
        };
        if this.format.sample_kind().is_none() || this.format.sample_rate < 1.0 {
            return Err(TapError::Unavailable(format!("unsupported tap format {:?}", this.format)));
        }

        let tap_entry = dict(vec![
            (kAudioSubTapUIDKey, obj(tap_uuid)),
            (kAudioSubTapDriftCompensationKey, obj(NSNumber::new_bool(true))),
        ]);
        let mut entries: Vec<(&CStr, Retained<NSObject>)> = vec![
            (kAudioAggregateDeviceNameKey, obj(NSString::from_str("Zwisper Tap"))),
            (kAudioAggregateDeviceUIDKey, obj(NSUUID::new().UUIDString())),
            (kAudioAggregateDeviceIsPrivateKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceIsStackedKey, obj(NSNumber::new_bool(false))),
            (kAudioAggregateDeviceTapAutoStartKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceTapListKey, obj(NSArray::from_retained_slice(&[tap_entry]))),
        ];
        // VERIFY ON MAC: Apple's sample / AudioCap add the default output
        // device as main sub-device (clock source). If the aggregate fails to
        // start on some setups (e.g. AirPlay/Bluetooth output), retry without it.
        // Also verify the IOProc's buffers use the tap format read above
        // (rather than the output device's nominal rate).
        if let Some(out_uid) = default_output_uid() {
            let sub = dict(vec![(kAudioSubDeviceUIDKey, obj(out_uid.clone()))]);
            entries.push((kAudioAggregateDeviceMainSubDeviceKey, obj(out_uid)));
            entries.push((kAudioAggregateDeviceSubDeviceListKey, obj(NSArray::from_retained_slice(&[sub]))));
        }
        let description = dict(entries);
        // SAFETY: NSDictionary is toll-free bridged with CFDictionary; the
        // reference stays valid for the duration of the call.
        let cf: &CFDictionary = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
        let mut aggregate: AudioObjectID = 0;
        check(
            unsafe { AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)) },
            "AudioHardwareCreateAggregateDevice",
        )?;
        this.aggregate = aggregate;
        Ok(this)
    }
}

impl Drop for TapObjects {
    fn drop(&mut self) {
        // SAFETY: ids were created by us and are destroyed exactly once.
        unsafe {
            if self.aggregate != 0 {
                let s = AudioHardwareDestroyAggregateDevice(self.aggregate);
                if s != 0 {
                    tracing::warn!(status = %fourcc(s), "destroy aggregate device failed");
                }
            }
            if self.tap != 0 {
                let s = AudioHardwareDestroyProcessTap(self.tap);
                if s != 0 {
                    tracing::warn!(status = %fourcc(s), "destroy process tap failed");
                }
            }
        }
    }
}

impl PcmFormat {
    fn default_float(rate: f64, channels: u32) -> Self {
        PcmFormat {
            sample_rate: rate,
            format_id: crate::convert::FORMAT_LINEAR_PCM,
            flags: crate::convert::FLAG_IS_FLOAT,
            bits_per_channel: 32,
            channels_per_frame: channels,
        }
    }
}

/// Everything the IOProc touches. Owned by `CoreAudioTap` through a raw
/// pointer for the lifetime of the IOProc registration.
struct CallbackState {
    sink: FrameSink,
    kind: SampleKind,
    resampler: Resampler,
    decoded: Vec<f32>,
    planar: Vec<Vec<f32>>,
    mixed: Vec<f32>,
}

impl CallbackState {
    /// # Safety
    /// `list` must point to a valid AudioBufferList with `mNumberBuffers` entries.
    unsafe fn handle(&mut self, list: *const AudioBufferList) {
        let n = (*list).mNumberBuffers as usize;
        if n == 0 {
            return;
        }
        // The buffers follow the header contiguously (C flexible array).
        let first = std::ptr::addr_of!((*list).mBuffers) as *const AudioBuffer;
        let bufs = std::slice::from_raw_parts(first, n);
        let bytes_of = |b: &AudioBuffer| -> &[u8] {
            if b.mData.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(b.mData as *const u8, b.mDataByteSize as usize)
            }
        };
        let mono: &[f32] = if n == 1 {
            // Interleaved: one buffer carrying all channels.
            self.decoded.clear();
            decode_into(bytes_of(&bufs[0]), self.kind, &mut self.decoded);
            let ch = bufs[0].mNumberChannels.max(1) as usize;
            if ch > 1 {
                self.mixed = downmix(&self.decoded, ch);
                &self.mixed
            } else {
                &self.decoded
            }
        } else {
            // Non-interleaved: one buffer per channel.
            self.planar.resize_with(n, Vec::new);
            for (b, dst) in bufs.iter().zip(self.planar.iter_mut()) {
                dst.clear();
                decode_into(bytes_of(b), self.kind, dst);
                let ch = b.mNumberChannels.max(1) as usize;
                if ch > 1 {
                    *dst = downmix(dst, ch);
                }
            }
            let refs: Vec<&[f32]> = self.planar.iter().map(|v| v.as_slice()).collect();
            mix_planar(&refs, &mut self.mixed);
            &self.mixed
        };
        if mono.is_empty() {
            return;
        }
        let out = self.resampler.process(mono);
        if !out.is_empty() {
            (self.sink)(&out);
        }
    }
}

/// The IOProc. Runs on the HAL's real-time IO thread.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> OSStatus {
    if client.is_null() {
        return 0;
    }
    // Never unwind into CoreAudio.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `client` is the Box<CallbackState> registered in `start`,
        // which outlives the IOProc (freed only after AudioDeviceDestroyIOProcID).
        // The HAL never runs one IOProc concurrently with itself.
        let state = &mut *(client as *mut CallbackState);
        state.handle(input.as_ptr());
    }));
    0
}

/// System-output capture via a process tap + private aggregate device.
pub struct CoreAudioTap {
    objects: Option<TapObjects>,
    proc_id: AudioDeviceIOProcID,
    state: *mut CallbackState,
}

// SAFETY: the raw pointer is only dereferenced by the IOProc while registered;
// `CoreAudioTap` itself is only touched through `&mut self`. The HAL APIs used
// in start/stop are thread-safe.
unsafe impl Send for CoreAudioTap {}

impl CoreAudioTap {
    /// Create the tap and aggregate device now, so permission / availability
    /// problems surface from `system_output()` rather than `start()`.
    pub(crate) fn new() -> std::result::Result<Self, TapError> {
        Ok(CoreAudioTap { objects: Some(TapObjects::create()?), proc_id: None, state: std::ptr::null_mut() })
    }

    fn teardown_io(&mut self) {
        if let Some(objs) = &self.objects {
            if self.proc_id.is_some() {
                // SAFETY: proc_id was created on this aggregate device.
                unsafe {
                    let _ = AudioDeviceStop(objs.aggregate, self.proc_id);
                    let _ = AudioDeviceDestroyIOProcID(objs.aggregate, self.proc_id);
                }
                self.proc_id = None;
            }
        }
        if !self.state.is_null() {
            // SAFETY: the IOProc is destroyed above, so nothing else references the state.
            // VERIFY ON MAC: AudioDeviceDestroyIOProcID is documented to wait for
            // an in-flight IO cycle to finish; if not, a short sleep is needed here.
            drop(unsafe { Box::from_raw(self.state) });
            self.state = std::ptr::null_mut();
        }
    }
}

fn tap_err(e: TapError) -> Error {
    match e {
        TapError::Permission => Error::Permission(PermissionKind::SystemAudio),
        TapError::Unavailable(m) => Error::Audio(m),
    }
}

impl AudioSource for CoreAudioTap {
    fn kind(&self) -> TrackKind {
        TrackKind::System
    }

    fn start(&mut self, sink: FrameSink) -> Result<()> {
        self.teardown_io();
        if self.objects.is_none() {
            self.objects = Some(TapObjects::create().map_err(tap_err)?);
        }
        let objs = self.objects.as_ref().expect("created above");
        let kind = objs.format.sample_kind().expect("validated in create");
        tracing::info!(format = ?objs.format, "starting system audio tap");
        let state = Box::into_raw(Box::new(CallbackState {
            sink,
            kind,
            resampler: Resampler::new(objs.format.sample_rate.round() as u32),
            decoded: Vec::with_capacity(8192),
            planar: Vec::new(),
            mixed: Vec::with_capacity(8192),
        }));
        self.state = state;
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `io_proc` matches AudioDeviceIOProc; `state` stays alive until teardown_io.
        let status = unsafe {
            AudioDeviceCreateIOProcID(objs.aggregate, Some(io_proc), state as *mut c_void, NonNull::from(&mut proc_id))
        };
        if let Err(e) = check(status, "AudioDeviceCreateIOProcID") {
            self.teardown_io();
            return Err(tap_err(e));
        }
        self.proc_id = proc_id;
        // SAFETY: valid device + proc id.
        let status = unsafe { AudioDeviceStart(objs.aggregate, self.proc_id) };
        if let Err(e) = check(status, "AudioDeviceStart") {
            self.teardown_io();
            return Err(tap_err(e));
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.teardown_io();
        // Release the tap + aggregate as well; `start` recreates them if needed.
        self.objects = None;
    }
}

impl Drop for CoreAudioTap {
    fn drop(&mut self) {
        self.stop();
    }
}
