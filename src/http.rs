//! HTTP routing and handlers.
//!
//! # Health contract
//!
//! `GET /api/health` is **public** and responds with HTTP `200` and
//! `Content-Type: application/json`. The body is a JSON object exposing the
//! service status together with the application name and version resolved at
//! compile time from `Cargo.toml`:
//!
//! ```json
//! { "status": "ok", "name": "apimail", "version": "0.1.0" }
//! ```
//!
//! # Authentication contract
//!
//! Every route **except** `GET /api/health` is protected by the
//! [`require_api_key`] middleware. A request must carry the configured API key
//! in the `Authorization` header using the `Bearer` scheme (the scheme is
//! matched case-insensitively):
//!
//! ```text
//! Authorization: Bearer <APIMAIL_API_KEY>
//! ```
//!
//! The configured key is never compared directly. Instead a SHA-256 digest of
//! the key is precomputed once at startup and the digest of the incoming token
//! is compared against it with `subtle`'s constant-time equality over a
//! **fixed-size** `[u8; 32]` buffer. Because both operands always have the same
//! length, the comparison cannot leak the secret's length through timing; the
//! only variable cost is hashing the attacker-supplied token, which depends
//! solely on the input the attacker already controls.
//!
//! Between the scheme and the token any run of ASCII spaces or tabs is accepted,
//! but whitespace inside the token is not.
//!
//! - `GET /api/whoami` is a protected scaffolding route. On success it returns
//!   HTTP `200`, `Content-Type: application/json` and the body
//!   `{"authenticated":true}`.
//! - Any missing header, non-`Bearer` scheme or wrong token is rejected with
//!   HTTP `401`, `Content-Type: application/json`, the body
//!   `{"error":"unauthorized","message":"..."}` and the challenge header
//!   `WWW-Authenticate: Bearer`.
//!
//! Requests for unknown routes still return `404`, and requests using an
//! unhandled method (for example `POST /api/health`) still return `405`; the
//! authentication middleware is layered only over the protected sub-router.

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::Config;

/// Application name, resolved from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
/// Application version, resolved from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared state exposed to handlers.
///
/// The configured API key is stored only as its SHA-256 digest and the field is
/// private, so the plaintext secret can neither be read nor formatted. The
/// manual [`Debug`] implementation redacts the digest as well.
#[derive(Clone)]
pub struct AppState {
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
    /// SHA-256 digest of the expected API key.
    api_key_hash: [u8; 32],
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("api_key_hash", &"***")
            .finish()
    }
}

impl AppState {
    /// Builds the application state from the loaded [`Config`].
    ///
    /// The expected key is hashed to a fixed-size digest here (not on every
    /// request) so the middleware can compare two equal-length buffers in
    /// constant time. [`Config`] guarantees a non-empty key; the assertion below
    /// documents that invariant in debug builds.
    pub fn from_config(config: &Config) -> Self {
        debug_assert!(
            !config.api_key.is_empty(),
            "AppState must not be built with an empty API key"
        );
        Self {
            name: APP_NAME,
            version: APP_VERSION,
            api_key_hash: Sha256::digest(config.api_key.as_bytes()).into(),
        }
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

/// JSON payload returned by `GET /api/whoami`.
#[derive(Debug, Serialize)]
struct WhoamiResponse {
    /// Always `true`: reached only after successful authentication.
    authenticated: bool,
}

/// Rejection returned when a request is not authenticated.
///
/// This is the embryo of the unified error model: a dedicated type implementing
/// [`IntoResponse`]. It renders the `401` contract (JSON body plus the
/// `Bearer` challenge).
#[derive(Debug)]
struct Unauthorized;

/// JSON body serialised for a `401` response.
#[derive(Debug, Serialize)]
struct UnauthorizedResponse {
    /// Stable machine-readable error code.
    error: &'static str,
    /// Human-readable explanation.
    message: &'static str,
}

impl IntoResponse for Unauthorized {
    fn into_response(self) -> Response {
        let body = Json(UnauthorizedResponse {
            error: "unauthorized",
            message: "missing or invalid API key",
        });
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            body,
        )
            .into_response()
    }
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        name: state.name,
        version: state.version,
    })
}

async fn whoami() -> Json<WhoamiResponse> {
    Json(WhoamiResponse {
        authenticated: true,
    })
}

/// Authorization middleware: rejects requests without a valid `Bearer` API key.
async fn require_api_key(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if is_authorized(&request, &state.api_key_hash) {
        next.run(request).await
    } else {
        Unauthorized.into_response()
    }
}

/// Returns `true` when the request carries the expected key as a `Bearer` token.
///
/// The token is hashed to a fixed-size digest and compared against the expected
/// digest in constant time.
fn is_authorized(request: &Request, expected_hash: &[u8; 32]) -> bool {
    let Some(value) = request.headers().get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = bearer_token(value) else {
        return false;
    };
    let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    candidate.ct_eq(expected_hash).into()
}

/// Extracts the token from an `Authorization` header value using the `Bearer`
/// scheme (matched case-insensitively).
///
/// The scheme and the token may be separated by one or more ASCII spaces or
/// tabs; trailing separators are ignored. Whitespace inside the token is
/// rejected, and an empty token is not accepted.
fn bearer_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, rest) = value.split_once([' ', '\t'])?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim_matches([' ', '\t']);
    if token.is_empty() || token.contains([' ', '\t']) {
        return None;
    }
    Some(token)
}

/// Builds the application [`Router`] with all routes mounted.
///
/// The returned router is self-contained and can be exercised in tests via
/// `tower::ServiceExt::oneshot` without binding a socket. `GET /api/health`
/// stays public; every other route sits behind [`require_api_key`].
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new().route("/api/whoami", get(whoami)).route_layer(
        middleware::from_fn_with_state(state.clone(), require_api_key),
    );

    Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .with_state(state)
}
