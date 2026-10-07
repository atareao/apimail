//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;

pub use config::{Config, ConfigError};
pub use http::{AppState, build_router};
