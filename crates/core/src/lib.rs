//! Zwisper core: domain types, ports (traits), services (use cases), the
//! command/event bus and cross-cutting decorators.
//!
//! This crate must never depend on an engine, a UI toolkit or an OS API.
//! Concrete strategies live in `crates/adapters/*` and are chosen by the
//! composition root in `crates/app`.

pub mod align;
pub mod bus;
pub mod cancel;
pub mod capture;
pub mod controller;
pub mod domain;
pub mod dsp;
pub mod echo;
pub mod engines;
pub mod error;
pub mod lazy;
pub mod ports;
pub mod services;
pub mod swap;
pub mod testkit;

pub use bus::{Command, DictationPhase, Event, EventBus};
pub use cancel::CancelToken;
pub use controller::Core;
pub use error::{Error, Result};
pub use services::Ports;
