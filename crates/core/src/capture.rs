//! Moves frames off the (possibly real-time) audio callback onto a worker
//! thread that runs a `FrameProcessor`.

use crate::error::Result;
use crate::ports::AudioSource;
use crossbeam_channel::{bounded, RecvTimeoutError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub trait FrameProcessor: Send + 'static {
    fn process(&mut self, frames: &[f32]);
    /// End of stream. Flush buffers, close files.
    fn finish(&mut self) {}
}

pub struct Capture<P: FrameProcessor> {
    source: Box<dyn AudioSource>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<P>>,
}

impl<P: FrameProcessor> Capture<P> {
    pub fn start(mut source: Box<dyn AudioSource>, mut processor: P) -> Result<Self> {
        // ~40 s of 10 ms buffers: plenty of slack while a chunk is transcribed.
        let (tx, rx) = bounded::<Vec<f32>>(4096);
        source.start(Box::new(move |f: &[f32]| {
            // Never block the audio thread; drop on overflow.
            let _ = tx.try_send(f.to_vec());
        }))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("capture-{:?}", source.kind()))
            .spawn(move || {
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(v) => processor.process(&v),
                        Err(RecvTimeoutError::Timeout) => {
                            if stop2.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                for v in rx.try_iter() {
                    processor.process(&v);
                }
                processor.finish();
                processor
            })
            .map_err(|e| crate::Error::Audio(e.to_string()))?;
        Ok(Capture { source, stop, thread: Some(thread) })
    }

    /// Stop the device, drain, finish and hand the processor back.
    pub fn stop(mut self) -> P {
        self.source.stop();
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().expect("capture thread panicked")
    }
}
