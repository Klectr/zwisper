//! Microphone capture through cpal's CoreAudio host.

use crate::convert::MonoConverter;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use zwisper_core::domain::{PermissionKind, TrackKind};
use zwisper_core::ports::{AudioSource, FrameSink};
use zwisper_core::{Error, Result};

/// Human-readable name used to select a device (matches `list_microphones`).
pub(crate) fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| d.to_string())
}

pub(crate) fn list_input_names() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devs) => devs.map(|d| device_name(&d)).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "could not enumerate input devices");
            Vec::new()
        }
    }
}

fn find_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_input_device()
            .ok_or_else(|| Error::Audio("no default input device".into())),
        Some(n) => host
            .input_devices()
            .map_err(map_cpal_err)?
            .find(|d| device_name(d) == n)
            .ok_or_else(|| Error::NotFound(format!("input device '{n}'"))),
    }
}

pub(crate) fn map_cpal_err(e: cpal::Error) -> Error {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => Error::Permission(PermissionKind::Microphone),
        _ => Error::Audio(e.to_string()),
    }
}

/// A cpal input stream. The `cpal::Stream` lives on a small dedicated thread
/// (it isn't guaranteed `Send` on every platform, and this keeps `CpalMic`
/// trivially `Send`); audio callbacks still run on CoreAudio's real-time thread.
pub struct CpalMic {
    device: Option<String>,
    worker: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}

impl CpalMic {
    pub(crate) fn new(device: Option<&str>) -> Result<Self> {
        // Validate the device now so `microphone()` fails early on a bad name.
        find_device(device)?;
        Ok(CpalMic {
            device: device.map(str::to_owned),
            worker: None,
        })
    }
}

fn build_stream<T>(device: &cpal::Device, config: cpal::StreamConfig, mut sink: FrameSink) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut conv = MonoConverter::new(config.sample_rate, config.channels as usize);
    // Scratch buffer reused across callbacks (grows at most a few times).
    let mut scratch: Vec<f32> = Vec::with_capacity(8192);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                // Real-time thread: no locks, no blocking. The sink is cheap by contract.
                scratch.clear();
                scratch.extend(data.iter().map(|&s| <f32 as FromSample<T>>::from_sample_(s)));
                let out = conv.process_interleaved(&scratch);
                if !out.is_empty() {
                    sink(&out);
                }
            },
            |e: cpal::Error| tracing::warn!(error = %e, "microphone stream error"),
            Some(Duration::from_secs(3)),
        )
        .map_err(map_cpal_err)
}

fn open_stream(device: Option<&str>, sink: FrameSink) -> Result<cpal::Stream> {
    let dev = find_device(device)?;
    let supported = dev.default_input_config().map_err(map_cpal_err)?;
    let format = supported.sample_format();
    let config = supported.config();
    tracing::info!(device = %device_name(&dev), rate = config.sample_rate, channels = config.channels, ?format, "opening microphone");
    let stream = match format {
        SampleFormat::F32 => build_stream::<f32>(&dev, config, sink),
        SampleFormat::F64 => build_stream::<f64>(&dev, config, sink),
        SampleFormat::I8 => build_stream::<i8>(&dev, config, sink),
        SampleFormat::I16 => build_stream::<i16>(&dev, config, sink),
        SampleFormat::I24 => build_stream::<cpal::I24>(&dev, config, sink),
        SampleFormat::I32 => build_stream::<i32>(&dev, config, sink),
        SampleFormat::I64 => build_stream::<i64>(&dev, config, sink),
        SampleFormat::U8 => build_stream::<u8>(&dev, config, sink),
        SampleFormat::U16 => build_stream::<u16>(&dev, config, sink),
        SampleFormat::U24 => build_stream::<cpal::U24>(&dev, config, sink),
        SampleFormat::U32 => build_stream::<u32>(&dev, config, sink),
        SampleFormat::U64 => build_stream::<u64>(&dev, config, sink),
        other => Err(Error::Audio(format!("unsupported sample format {other:?}"))),
    }?;
    stream.play().map_err(map_cpal_err)?;
    Ok(stream)
}

impl AudioSource for CpalMic {
    fn kind(&self) -> TrackKind {
        TrackKind::Mic
    }

    fn start(&mut self, sink: FrameSink) -> Result<()> {
        self.stop();
        let device = self.device.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = std::thread::Builder::new()
            .name("zwisper-mic".into())
            .spawn(move || match open_stream(device.as_deref(), sink) {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    // Park until stop() (or the sender is dropped), then drop the stream.
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| Error::Audio(format!("spawn mic thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.worker = Some((stop_tx, handle));
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(Error::Audio("microphone thread exited unexpectedly".into()))
            }
        }
        // VERIFY ON MAC: when Microphone permission is denied, CoreAudio
        // typically still opens the stream and delivers silence rather than
        // failing. The app should check `Permissions::state(Microphone)` before
        // starting, which is why `MacPermissions` exists.
    }

    fn stop(&mut self) {
        if let Some((tx, handle)) = self.worker.take() {
            let _ = tx.send(());
            let _ = handle.join();
        }
    }
}

impl Drop for CpalMic {
    fn drop(&mut self) {
        self.stop();
    }
}
