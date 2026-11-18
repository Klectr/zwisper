//! Zwisper's GPUI front-end. Depends only on `zwisper-core`: it sends
//! `Command`s and renders `Event`s. Window chrome is calm and minimal.

pub mod actions;
pub mod editor;
pub mod library;
pub mod markdown;
pub mod meeting;
pub mod onboarding;
pub mod pill;
pub mod settings;
pub mod state;
pub mod theme;
pub mod widgets;

pub use actions::{open_library, open_settings, start_meeting, toggle_dictation};
pub use state::{install, AppState, UiPlatform};
