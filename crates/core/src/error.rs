use crate::domain::{Pack, PermissionKind};

/// The single error type that crosses port boundaries. Adapters map their
/// library-specific errors into one of these variants; nothing above the
/// adapter layer ever sees an ONNX, llama.cpp or CoreAudio error type.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("the {} model pack is not downloaded", .0.label())]
    ModelMissing(Pack),
    #[error("permission needed: {0:?}")]
    Permission(PermissionKind),
    #[error("audio device error: {0}")]
    Audio(String),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
