use crate::domain::Tier;
use crate::error::Result;
use crate::ports::*;
use crate::swap::Swappable;
use std::sync::Arc;

/// The currently active AI strategies. Each slot can be hot-swapped.
pub struct Engines {
    pub transcriber: Swappable<dyn Transcriber>,
    pub diarizer: Swappable<dyn Diarizer>,
    pub notes: Swappable<dyn NoteWriter>,
    factory: Arc<dyn EngineFactory>,
}

impl Engines {
    pub fn build(factory: Arc<dyn EngineFactory>, tier: Tier) -> Result<Self> {
        Ok(Engines {
            transcriber: Swappable::new(factory.transcriber(tier)?),
            diarizer: Swappable::new(factory.diarizer(tier)?),
            notes: Swappable::new(factory.note_writer(tier)?),
            factory,
        })
    }

    /// A fresh VAD (one per track; VADs are stateful).
    pub fn new_vad(&self) -> Result<Box<dyn VoiceActivity>> {
        self.factory.vad()
    }

    /// Swap every slot to the strategies for `tier`.
    pub fn retier(&self, tier: Tier) -> Result<()> {
        self.transcriber.swap(self.factory.transcriber(tier)?);
        self.diarizer.swap(self.factory.diarizer(tier)?);
        self.notes.swap(self.factory.note_writer(tier)?);
        Ok(())
    }

    /// Periodic housekeeping (idle unload). Skips engines that are busy.
    pub fn maintain(&self) {
        if let Some(mut t) = self.transcriber.current().try_lock() {
            t.maintain();
        }
        if let Some(mut d) = self.diarizer.current().try_lock() {
            d.maintain();
        }
        if let Some(mut n) = self.notes.current().try_lock() {
            n.maintain();
        }
    }
}
