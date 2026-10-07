//! HTTP routing and handlers.
//!
//! # Health contract
//!
//! `GET /api/health` responds with HTTP `200` and `Content-Type:
//! application/json`. The body is a JSON object exposing the service status
//! together with the application name and version resolved at compile time
//! from `Cargo.toml`:
//!
//! ```json
//! { "status": "ok", "name": "apimail", "version": "0.1.0" }
//! ```
//!
//! Requests for unknown routes return `404`, and requests using an unhandled
//! method (for example `POST /api/health`) return `405`.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// Application name, resolved from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
/// Application version, resolved from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared state exposed to handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
}

impl AppState {
    /// Builds the default application state from the compiled crate metadata.
    pub fn new() -> Self {
        Self {
            name: APP_NAME,
            version: APP_VERSION,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON payload returned by `GET /api/health`.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Liveness status marker.
    pub status: &'static str,
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        name: state.name,
        version: state.version,
    })
}

/// Builds the application [`Router`] with all routes mounted.
///
/// The returned router is self-contained and can be exercised in tests via
/// `tower::ServiceExt::oneshot` without binding a socket.
pub fn build_router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .with_state(AppState::new())
}
