//! The real engine strategies, each wrapped in `Lazy` so models load on first
//! use and unload when idle. Model files come from the `ModelStore`.

use std::sync::Arc;
use std::time::Duration;
use zwisper_core::domain::{Pack, Tier};
use zwisper_core::lazy::Lazy;
use zwisper_core::ports::*;
use zwisper_core::Result;

pub struct RealEngines {
    pub models: Arc<dyn ModelStore>,
}

const IDLE: Duration = Duration::from_secs(5 * 60);

impl EngineFactory for RealEngines {
    fn transcriber(&self, tier: Tier) -> Result<Box<dyn Transcriber>> {
        let m = self.models.clone();
        Ok(Box::new(
            Lazy::new("parakeet-tdt-0.6b-v2", move || zwisper_sherpa::ParakeetTranscriber::load(&m.paths(Pack::Speech, tier)?))
                .with_idle_timeout(IDLE),
        ))
    }

    fn vad(&self) -> Result<Box<dyn VoiceActivity>> {
        // Fall back to the energy VAD if the Speech pack is missing; the
        // transcriber will report the missing model to the user.
        match self.models.paths(Pack::Speech, Tier::Standard) {
            Ok(p) => Ok(Box::new(zwisper_sherpa::SileroVad::load(&p)?)),
            Err(_) => Ok(Box::new(zwisper_core::dsp::EnergyVad::new(0.01))),
        }
    }

    fn diarizer(&self, tier: Tier) -> Result<Box<dyn Diarizer>> {
        let m = self.models.clone();
        Ok(Box::new(Lazy::new("pyannote-seg3+wespeaker-r34", move || {
            zwisper_sherpa::SherpaDiarizer::load(&m.paths(Pack::Speakers, tier)?)
        })))
    }

    fn note_writer(&self, tier: Tier) -> Result<Box<dyn NoteWriter>> {
        let m = self.models.clone();
        let variant = match tier {
            Tier::Standard => zwisper_llama::Variant::Standard,
            Tier::Lite => zwisper_llama::Variant::Lite,
        };
        Ok(Box::new(
            Lazy::new(variant.engine_id(), move || zwisper_llama::LlamaNoteWriter::load(&m.paths(Pack::Notes, tier)?, variant))
                .with_idle_timeout(IDLE),
        ))
    }
}
