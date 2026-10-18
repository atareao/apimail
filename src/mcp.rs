//! MCP (Model Context Protocol) server for `apimail`, served over standard input
//! and output or over Streamable HTTP.
//!
//! The whole module lives behind the optional `mcp` Cargo feature, so a default
//! build neither compiles `rmcp`/`schemars` nor exposes any MCP surface.
//!
//! Its scope is deliberately minimal: **one** tool, `whoami`, whose only job is
//! to prove the complete cycle (`initialize` → `tools/list` → `tools/call`) runs
//! over both transports on top of the shared [`AppState`]. The mail tools come
//! later.
//!
//! # Standard output is protocol-only
//!
//! A stdio MCP server speaks newline-delimited JSON-RPC over its standard
//! output, so nothing else may be written there or the client sees a corrupt
//! stream. Every diagnostic therefore goes to standard error (the binary already
//! points `tracing` at `stderr`), and this module never prints anything itself:
//! it hands the process streams to `rmcp` and stays out of the way.
//!
//! # Streamable HTTP
//!
//! [`streamable_http_service`] builds the same [`ApimailMcp`] handler as a Tower
//! service, which [`build_router`](crate::build_router) nests **inside** the
//! protected sub-router of the HTTP API. That placement is the whole point: the
//! MCP endpoint shares the existing listener — no second port — and inherits the
//! existing `require_api_key` middleware, so both transports authenticate against
//! the same digest through `AppState::verify_token` instead of a second, weaker
//! check (a code span rather than a link: the method is crate-private, so rustdoc
//! rejects the link from public docs).

use std::sync::Arc;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Json},
    model::{Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Serialize;

use crate::config::Config;
use crate::service::{self, AppState, AppStateError};

/// Structured result of the `whoami` tool.
///
/// It mirrors [`service::whoami`]'s single fact, field for field. The mirror is
/// deliberate: the service DTOs carry no `schemars` derive yet (they are shared
/// with the HTTP layer and only the MCP tool surface needs schemas), so the
/// schema-bearing view lives here rather than in `src/service.rs`, which keeps
/// the HTTP contract untouched under both builds.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct WhoamiOutput {
    /// Always `true`: the call reached the tool only after authentication.
    pub authenticated: bool,
}

impl From<service::WhoamiResponse> for WhoamiOutput {
    fn from(response: service::WhoamiResponse) -> Self {
        Self {
            authenticated: response.authenticated,
        }
    }
}

/// The `apimail` MCP server: the [`AppState`] plus the router of its tools.
///
/// Holds a clone of the shared state — [`AppState`] is `Clone` and cheap to
/// clone — so a future tool can delegate to the same service operations the HTTP
/// handlers use, with no second copy of the business logic.
#[derive(Clone)]
pub struct ApimailMcp {
    /// Shared application state (API key digest, mail account, services).
    state: AppState,
    /// Router generated from the `#[tool]` methods below.
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ApimailMcp {
    /// Builds the server over an already-constructed [`AppState`].
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Reports whether the caller is authenticated.
    ///
    /// Delegates to [`service::whoami`], which is infallible and never touches
    /// the network: the single fact it can assert is that the call was already
    /// authenticated by the transport.
    #[tool(
        description = "Report whether the caller is authenticated. Always returns `authenticated: true`, because the call did not fail authentication."
    )]
    async fn whoami(&self) -> Json<WhoamiOutput> {
        Json(service::whoami().into())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ApimailMcp {
    /// Advertises the tool capability and names this build as the server.
    ///
    /// Overrides the macro-generated version so the identity comes from the
    /// shared [`AppState`] (`name` and `version`) instead of the crate's build
    /// environment, keeping one source of truth for both transports.
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(self.state.name, self.state.version))
            .with_instructions(
                "apimail exposes a mail account over MCP. Call `whoami` to confirm the \
                 connection is authenticated.",
            )
    }
}

/// Builds the MCP server as a Tower service for the Streamable HTTP transport.
///
/// The returned service is meant to be nested in the HTTP API's router with
/// `nest_service(path, …)`, which is what makes the MCP endpoint share the
/// existing listener and the existing `require_api_key` middleware. It binds no
/// port of its own and every handler it creates is built from the same
/// [`AppState`], so a tool call over HTTP sees exactly the services (and the same
/// API key digest) a tool call over stdio sees.
///
/// The factory is called once per session rather than once at startup, which is
/// why [`ApimailMcp`] is `Clone`: cloning the handler clones an `AppState` (a
/// handful of `Arc`s) and the generated `ToolRouter`.
///
/// Sessions are tracked in memory ([`LocalSessionManager`]) with the SDK's
/// default transport configuration, so a client's `initialize` gets a
/// `Mcp-Session-Id` it must echo on every later request.
pub fn streamable_http_service(
    state: AppState,
) -> StreamableHttpService<ApimailMcp, LocalSessionManager> {
    let sessions = Arc::new(LocalSessionManager::default());
    StreamableHttpService::new(
        move || Ok(ApimailMcp::new(state.clone())),
        sessions,
        StreamableHttpServerConfig::default(),
    )
}

/// Serves the MCP server over the process's standard input and output.
///
/// Builds the [`AppState`] from `config` — which keeps startup fail-closed in
/// stdio mode exactly as it is for HTTP: a missing `APIMAIL_API_KEY` or an
/// incomplete mail account aborts before a single byte of protocol is written —
/// and then serves the stdio transport until the peer disconnects or the
/// process is asked to stop. No TCP port is bound.
///
/// # Errors
///
/// [`McpServeError::State`] when the application state cannot be built, and
/// [`McpServeError::Handshake`] / [`McpServeError::Task`] when the transport
/// fails or its task cannot be joined.
pub async fn serve_stdio(config: &Config) -> Result<(), McpServeError> {
    let state = AppState::from_config(config)?;
    let running = ApimailMcp::new(state)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(Box::new)?;
    let reason = running.waiting().await?;
    tracing::debug!(?reason, "MCP stdio transport finished");
    Ok(())
}

/// Failure to run the MCP server over stdio.
#[derive(Debug, thiserror::Error)]
pub enum McpServeError {
    /// The application state could not be built from the configuration.
    #[error(transparent)]
    State(#[from] AppStateError),
    /// The MCP handshake failed or the transport closed with an error.
    ///
    /// Boxed because `ServerInitializeError` is far larger than the other
    /// variants: keeping it inline would size the whole enum to it and make
    /// every `Result<(), McpServeError>` pay for it (`clippy::large_enum_variant`).
    /// [`Box`]'s `Display` delegates to the inner error, so the message is
    /// unchanged.
    #[error("MCP stdio transport failed: {0}")]
    Handshake(#[from] Box<rmcp::service::ServerInitializeError>),
    /// The server task ended abnormally.
    #[error("MCP stdio server task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl McpServeError {
    /// Stable machine-filterable tag, for **logs only**.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::State(_) => "state",
            Self::Handshake(_) => "handshake",
            Self::Task(_) => "task",
        }
    }
}
