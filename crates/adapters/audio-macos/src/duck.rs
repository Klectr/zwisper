//! `AudioDucker` for macOS: muffle (process tap + low-pass) or lower the
//! output volume while dictating.
//!
//! All Core Audio work happens on one worker thread, so `duck`/`restore`
//! return immediately and dictation never waits on the HAL.

use crate::muffle_tap::MuffleTap;
use crate::volume::{recover, Lowered};
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;
use zwisper_core::domain::Ducking;
use zwisper_core::ports::AudioDucker;

/// "Lower": scale the output volume by this.
const LOWER_FACTOR: f32 = 0.3;
/// Let the filter glide open before un-muting, so the sound fades back in.
const FADE_OUT: Duration = Duration::from_millis(200);

enum Msg {
    Duck(Ducking),
    Restore,
}

pub struct MacDucker {
    tx: Mutex<Sender<Msg>>,
}

impl MacDucker {
    /// `state_file` remembers a lowered volume so a crash can't leave it low.
    /// `system_audio_allowed` reports the System Audio Recording permission;
    /// without it Muffle falls back to Lower (we never trigger the prompt
    /// mid-dictation).
    pub fn new(state_file: PathBuf, system_audio_allowed: Box<dyn Fn() -> bool + Send>) -> Self {
        let (tx, rx) = channel::<Msg>();
        let spawned = std::thread::Builder::new().name("zwisper-ducker".into()).spawn(move || {
            recover(&state_file);
            let mut worker = Worker { state_file, allowed: system_audio_allowed, muffle: None, lowered: None };
            let mut next = rx.recv().ok();
            while let Some(msg) = next.take() {
                match msg {
                    Msg::Duck(how) => worker.duck(how),
                    Msg::Restore => {
                        if let Some(tap) = worker.muffle.take() {
                            tap.set_target(0.0);
                            match rx.recv_timeout(FADE_OUT) {
                                // Dictation started again mid-fade: just close the door again.
                                Ok(Msg::Duck(Ducking::Muffle)) => {
                                    tap.set_target(1.0);
                                    worker.muffle = Some(tap);
                                }
                                Ok(other) => {
                                    drop(tap);
                                    next = Some(other);
                                }
                                Err(RecvTimeoutError::Timeout) => drop(tap),
                                Err(RecvTimeoutError::Disconnected) => drop(tap),
                            }
                        }
                        worker.restore_volume();
                    }
                }
                if next.is_none() {
                    next = rx.recv().ok();
                }
            }
            // Ducker dropped: never leave things quiet.
            worker.muffle = None;
            worker.restore_volume();
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the ducking thread; other audio won't be quieted");
        }
        MacDucker { tx: Mutex::new(tx) }
    }

    fn send(&self, m: Msg) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(m);
        }
    }
}

impl AudioDucker for MacDucker {
    fn duck(&self, how: Ducking) {
        self.send(Msg::Duck(how));
    }
    fn restore(&self) {
        self.send(Msg::Restore);
    }
}

struct Worker {
    state_file: PathBuf,
    allowed: Box<dyn Fn() -> bool + Send>,
    muffle: Option<MuffleTap>,
    lowered: Option<Lowered>,
}

impl Worker {
    fn duck(&mut self, how: Ducking) {
        if self.muffle.is_some() || self.lowered.is_some() {
            return; // already quiet
        }
        if how == Ducking::Muffle {
            if (self.allowed)() {
                match MuffleTap::start() {
                    Ok(tap) => {
                        tap.set_target(1.0);
                        self.muffle = Some(tap);
                        return;
                    }
                    Err(e) => tracing::warn!(error = %e, "muffle unavailable; lowering the volume instead"),
                }
            } else {
                tracing::info!("System Audio permission not granted; lowering the volume instead of muffling");
            }
        }
        if how != Ducking::Off {
            if let Some(l) = Lowered::apply(LOWER_FACTOR) {
                l.save(&self.state_file);
                self.lowered = Some(l);
            }
        }
    }

    fn restore_volume(&mut self) {
        if let Some(l) = self.lowered.take() {
            l.restore();
            let _ = std::fs::remove_file(&self.state_file);
        }
    }
}
