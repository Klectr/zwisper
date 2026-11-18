pub mod dictation;
pub mod library;
pub mod meeting;
pub mod models;
pub mod notes;
pub mod pipeline;

use crate::bus::EventBus;
use crate::engines::Engines;
use crate::ports::*;
use std::sync::Arc;

/// Every port the services need, injected by the composition root.
#[derive(Clone)]
pub struct Ports {
    pub audio: Arc<dyn AudioSourceFactory>,
    pub recorder: Arc<dyn AudioRecorder>,
    pub injector: Arc<dyn TextInjector>,
    pub repo: Arc<dyn SessionRepository>,
    pub settings: Arc<dyn SettingsStore>,
    pub models: Arc<dyn ModelStore>,
    pub templates: Arc<dyn TemplateCatalog>,
    pub aligner: Arc<dyn SpeakerAligner>,
    pub echo: Arc<dyn EchoFilter>,
    pub clock: Arc<dyn Clock>,
    pub engines: Arc<dyn EngineFactory>,
    /// Physical memory, used to pick the default tier.
    pub system_memory_bytes: u64,
}

/// Shared context handed to every service.
pub struct Ctx {
    pub ports: Ports,
    pub engines: Engines,
    pub events: EventBus,
}

pub type SharedCtx = Arc<Ctx>;
