//! MCP (Model Context Protocol) server for `apimail`, served over standard input
//! and output or over Streamable HTTP.
//!
//! The whole module lives behind the optional `mcp` Cargo feature, so a default
//! build neither compiles `rmcp`/`schemars` nor exposes any MCP surface.
//!
//! # Tool surface
//!
//! The server exposes the account's **read** operations plus `whoami`: twelve
//! tools in total. Every one of them is a thin `#[tool]` method that delegates to
//! the very same [`service`] operation its HTTP route calls and owns no business
//! logic at all — what belongs to this transport is only the dialect of its
//! arguments (which value is optional, how a date or a format is spelled, where a
//! page size is bounded) and the rendering of a failure.
//!
//! The two administrative operations behind `POST /api/idle/start` and
//! `POST /api/idle/stop` are deliberately **not** exposed: an MCP client may
//! observe the subscription and its delivery queue, but starting or stopping it
//! over the protocol is out of scope.
//!
//! # Failure model
//!
//! Every fallible tool answers with `Err(String)` carrying
//! `"{kind}: {public_message}"` of the underlying [`ServiceError`] — the very two
//! values the HTTP layer writes into its `{"error","message"}` envelope, so both
//! transports name a failure with one vocabulary, and neither leaks a credential
//! nor third-party text. `rmcp` renders the `Err` branch of
//! [`IntoCallToolResult`](rmcp::handler::server::tool::IntoCallToolResult) as a
//! **tool** error (`isError: true`) rather than a protocol error, so the caller
//! reads the code instead of seeing the stream break.
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
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;

use crate::config::Config;
use crate::imap::{FetchFormat, SearchCriteria, SearchDate};
use crate::service::{self, AppState, AppStateError, ServiceError};

/// Page size `list_messages` applies when the caller names none.
///
/// Mirrored from the HTTP query, which owns the same default and the same bound:
/// `limit` defaults to `50` and must not exceed [`MAX_LIMIT`].
const DEFAULT_LIMIT: u32 = 50;

/// Largest page size `list_messages` accepts, as in the HTTP query.
const MAX_LIMIT: u32 = 200;

/// Renders a domain failure as a tool error carrying its stable code.
///
/// [`ServiceError::kind`] and [`ServiceError::public_message`] are the very two
/// values the HTTP layer writes into its `{"error","message"}` envelope, so the
/// two transports name a failure with one vocabulary. Both are credential-free
/// and free of third-party text, so nothing a peer said reaches a caller through
/// this rendering.
fn tool_error(error: &ServiceError) -> String {
    format!("{}: {}", error.kind(), error.public_message())
}

/// Renders a request that does not speak this transport's dialect.
///
/// The code is the HTTP layer's `invalid_request` and the message is fixed: the
/// offending value is never echoed back, exactly as at the HTTP boundary.
fn invalid_input(message: &str) -> String {
    format!("invalid_request: {message}")
}

/// Whether `value` carries a character that could break an IMAP command line.
///
/// Mirrored from the HTTP boundary, which rejects `CR`, `LF` and `NUL` before any
/// command is built: quoting already neutralises them, so this is the same
/// documented defence in depth rather than a second rule.
fn has_control_characters(value: &str) -> bool {
    value.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
}

/// Validates a mailbox name: free of control characters and non-blank.
fn validate_mailbox(mailbox: &str) -> Result<String, String> {
    if has_control_characters(mailbox) {
        return Err(invalid_input(
            "`mailbox` contains a forbidden control character",
        ));
    }
    let mailbox = mailbox.trim();
    if mailbox.is_empty() {
        return Err(invalid_input("`mailbox` must not be empty"));
    }
    Ok(mailbox.to_string())
}

/// Validates an optional free-text search filter.
fn validate_filter(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    match value {
        Some(value) if has_control_characters(&value) => Err(invalid_input(&format!(
            "`{field}` contains a forbidden control character"
        ))),
        other => Ok(other),
    }
}

/// Validates and parses an optional `YYYY-MM-DD` date filter.
fn validate_date(value: Option<String>, field: &str) -> Result<Option<SearchDate>, String> {
    match value {
        Some(value) => SearchDate::parse(&value)
            .map(Some)
            .map_err(|_| invalid_input(&format!("`{field}` must be `YYYY-MM-DD`"))),
        None => Ok(None),
    }
}

/// Applies the page-size default and bound of the HTTP dialect.
fn validate_limit(limit: Option<u32>) -> Result<u32, String> {
    match limit {
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => Ok(limit),
        Some(_) => Err(invalid_input("`limit` must be between 1 and 200")),
        None => Ok(DEFAULT_LIMIT),
    }
}

/// Validates the positive identifier the single-message operations expect.
///
/// The JSON dialect already guarantees a `u32`; only the zero left by the HTTP
/// path's "non-numeric or zero" rule has to be rejected here.
fn validate_uid(uid: u32) -> Result<u32, String> {
    if uid == 0 {
        Err(invalid_input("`uid` must be a positive integer"))
    } else {
        Ok(uid)
    }
}

/// The `format` argument of `get_message`.
///
/// The variants are spelled exactly as the HTTP query spells them (`summary` is
/// the default), so a client names the same format on both transports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FetchFormatInput {
    /// Metadata and envelope only.
    #[default]
    Summary,
    /// Metadata, envelope and the raw header block, base64-encoded.
    Headers,
    /// Metadata, envelope and the full RFC822 source, base64-encoded.
    Full,
}

impl From<FetchFormatInput> for FetchFormat {
    fn from(format: FetchFormatInput) -> Self {
        match format {
            FetchFormatInput::Summary => Self::Summary,
            FetchFormatInput::Headers => Self::Headers,
            FetchFormatInput::Full => Self::Full,
        }
    }
}

/// Arguments of the `select_mailbox` tool.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SelectMailboxInput {
    /// Mailbox to select, for example `INBOX`.
    pub mailbox: String,
}

impl SelectMailboxInput {
    /// Validates the argument into the domain value.
    fn into_mailbox(self) -> Result<String, String> {
        validate_mailbox(&self.mailbox)
    }
}

/// Arguments of the `list_messages` tool.
///
/// Every filter is optional, exactly as in the HTTP query, so an empty set
/// searches the whole mailbox (`ALL`).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListMessagesInput {
    /// Mailbox to search.
    pub mailbox: String,
    /// Matches the `From` header.
    pub from: Option<String>,
    /// Matches the `To` header.
    pub to: Option<String>,
    /// Matches the `Subject` header.
    pub subject: Option<String>,
    /// Matches text anywhere in the message (`BODY`).
    pub text: Option<String>,
    /// `SINCE` filter, as `YYYY-MM-DD`.
    pub since: Option<String>,
    /// `BEFORE` filter, as `YYYY-MM-DD`.
    pub before: Option<String>,
    /// `true` selects `SEEN`, `false` selects `UNSEEN`.
    pub seen: Option<bool>,
    /// `true` selects `FLAGGED`, `false` selects `UNFLAGGED`.
    pub flagged: Option<bool>,
    /// Page size (`1..=200`, default `50`).
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
    /// Page offset (default `0`).
    pub offset: Option<u32>,
}

impl ListMessagesInput {
    /// Validates the arguments into the operation's own pieces.
    fn into_parts(self) -> Result<(String, SearchCriteria, u32, u32), String> {
        let mailbox = validate_mailbox(&self.mailbox)?;
        let criteria = SearchCriteria {
            from: validate_filter(self.from, "from")?,
            to: validate_filter(self.to, "to")?,
            subject: validate_filter(self.subject, "subject")?,
            text: validate_filter(self.text, "text")?,
            since: validate_date(self.since, "since")?,
            before: validate_date(self.before, "before")?,
            seen: self.seen,
            flagged: self.flagged,
        };
        let limit = validate_limit(self.limit)?;
        let offset = self.offset.unwrap_or(0);
        Ok((mailbox, criteria, limit, offset))
    }
}

/// Arguments of the `get_message` tool.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GetMessageInput {
    /// Mailbox holding the message.
    pub mailbox: String,
    /// Unique identifier of the message within the mailbox.
    pub uid: u32,
    /// Requested format; `summary` when omitted.
    pub format: Option<FetchFormatInput>,
}

impl GetMessageInput {
    /// Validates the arguments into the operation's own pieces.
    fn into_parts(self) -> Result<(String, u32, FetchFormat), String> {
        let mailbox = validate_mailbox(&self.mailbox)?;
        let uid = validate_uid(self.uid)?;
        Ok((mailbox, uid, self.format.unwrap_or_default().into()))
    }
}

/// Arguments of the tools that address one message without choosing a format:
/// `get_message_body` and `list_attachments`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct MessageRefInput {
    /// Mailbox holding the message.
    pub mailbox: String,
    /// Unique identifier of the message within the mailbox.
    pub uid: u32,
}

impl MessageRefInput {
    /// Validates the arguments into the operation's own pieces.
    fn into_parts(self) -> Result<(String, u32), String> {
        Ok((validate_mailbox(&self.mailbox)?, validate_uid(self.uid)?))
    }
}

/// Arguments of the `get_attachment` tool.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GetAttachmentInput {
    /// Mailbox holding the message.
    pub mailbox: String,
    /// Unique identifier of the message within the mailbox.
    pub uid: u32,
    /// Positional identifier of the attachment; `0` is valid.
    pub id: u32,
}

impl GetAttachmentInput {
    /// Validates the arguments into the operation's own pieces.
    fn into_parts(self) -> Result<(String, u32, u32), String> {
        Ok((
            validate_mailbox(&self.mailbox)?,
            validate_uid(self.uid)?,
            self.id,
        ))
    }
}

/// The `apimail` MCP server: the [`AppState`] plus the router of its tools.
///
/// Holds a clone of the shared state — [`AppState`] is `Clone` and cheap to
/// clone — so every tool delegates to the same service operations the HTTP
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
    async fn whoami(&self) -> Json<service::WhoamiResponse> {
        Json(service::whoami())
    }

    /// Reports the configured account's endpoints.
    ///
    /// Delegates to [`service::account`], which is infallible and returns the
    /// non-sensitive part of the configuration: hosts and ports, never a
    /// username or a password.
    #[tool(
        description = "Report the configured mail endpoints: the IMAP host and port and the SMTP host and port. Credentials are never included."
    )]
    async fn get_account(&self) -> Json<service::AccountView> {
        Json(service::account(&self.state).clone())
    }

    /// Ensures a live IMAP session and reports the outcome.
    ///
    /// Thin wrapper over [`service::imap_status`], which connects lazily on first
    /// use. The failure branch is the shared tool error: a `503`
    /// `imap_unavailable`, never a credential.
    #[tool(
        description = "Report whether the IMAP session is alive, connecting lazily on first use. A failure is reported as the `imap_unavailable` error code, never as a credential."
    )]
    async fn imap_status(&self) -> Result<Json<service::ImapStatusResponse>, String> {
        service::imap_status(&self.state)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Lists the account's mailboxes.
    ///
    /// Thin wrapper over [`service::list_mailboxes`], which issues `LIST` on the
    /// shared session and never exposes a credential.
    #[tool(
        description = "List the account's mailboxes, each with its hierarchy delimiter and its IMAP attributes (for example `\\NoSelect`)."
    )]
    async fn list_mailboxes(&self) -> Result<Json<service::MailboxListResponse>, String> {
        service::list_mailboxes(&self.state)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Selects a mailbox and reports its status.
    ///
    /// Thin wrapper over [`service::select_mailbox`]. A missing mailbox arrives
    /// as the `mailbox_not_found` tool error and an unavailable session as
    /// `imap_unavailable`; the selection stays active for the tools that follow.
    #[tool(
        description = "Select a mailbox (IMAP SELECT) and report its message counts, unseen count, UID validity, next UID and flags. The selection stays active for later tool calls."
    )]
    async fn select_mailbox(
        &self,
        Parameters(input): Parameters<SelectMailboxInput>,
    ) -> Result<Json<service::MailboxStatusResponse>, String> {
        let mailbox = input.into_mailbox()?;
        service::select_mailbox(&self.state, mailbox)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Searches a mailbox and returns one page of message metadata.
    ///
    /// Thin wrapper over [`service::list_messages`], which composes `SELECT`,
    /// `SEARCH` and a summary `UID FETCH`. Every filter is optional; `limit`
    /// defaults to `50` and `offset` to `0`, exactly as in the HTTP query.
    #[tool(
        description = "Search one page of a mailbox's messages and return their metadata and envelopes. Filters: `from`, `to`, `subject`, `text`, `since`/`before` as `YYYY-MM-DD`, `seen` and `flagged` as booleans; `limit` (1..=200, default 50) and `offset` (default 0)."
    )]
    async fn list_messages(
        &self,
        Parameters(input): Parameters<ListMessagesInput>,
    ) -> Result<Json<service::MessageListResponse>, String> {
        let (mailbox, criteria, limit, offset) = input.into_parts()?;
        service::list_messages(&self.state, mailbox, criteria, limit, offset)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Fetches one message by UID.
    ///
    /// Thin wrapper over [`service::fetch_message`], which reads with `BODY.PEEK`
    /// so fetching never sets `\Seen`. The raw header block and the raw source
    /// travel base64-encoded inside the JSON result, so a hostile message can
    /// never inject a header.
    #[tool(
        description = "Fetch one message by UID and return its metadata and envelope. `format` is `summary` (default), `headers` (adds the raw header block) or `full` (adds the whole RFC 822 source); the raw blocks are base64-encoded. Reading never marks the message as seen."
    )]
    async fn get_message(
        &self,
        Parameters(input): Parameters<GetMessageInput>,
    ) -> Result<Json<service::MessageDetailResponse>, String> {
        let (mailbox, uid, format) = input.into_parts()?;
        service::fetch_message(&self.state, mailbox, uid, format)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Fetches one message and reports its derived text and HTML bodies.
    ///
    /// Thin wrapper over [`service::message_body`]. Each body is `null` when the
    /// message carries no such part and none can be derived; HTML is returned
    /// verbatim, so rendering it safely is the client's business.
    #[tool(
        description = "Fetch one message by UID and return its plain-text and HTML bodies (`null` when the message carries neither). HTML is returned verbatim."
    )]
    async fn get_message_body(
        &self,
        Parameters(input): Parameters<MessageRefInput>,
    ) -> Result<Json<service::MessageBodyResponse>, String> {
        let (mailbox, uid) = input.into_parts()?;
        service::message_body(&self.state, mailbox, uid)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Lists the attachments of one message, without their content.
    ///
    /// Thin wrapper over [`service::list_attachments`], which parses the message
    /// on the shared session and reports one entry per part with an identifier
    /// that `get_attachment` accepts.
    #[tool(
        description = "List the attachments of one message by UID: position, file name, content type, size, inline flag and content id. The content itself is only returned by `get_attachment`."
    )]
    async fn list_attachments(
        &self,
        Parameters(input): Parameters<MessageRefInput>,
    ) -> Result<Json<service::AttachmentListResponse>, String> {
        let (mailbox, uid) = input.into_parts()?;
        service::list_attachments(&self.state, mailbox, uid)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Fetches the decoded content of one attachment, base64-encoded.
    ///
    /// Thin wrapper over [`service::attachment`], which identifies the attachment
    /// by its positional `id` (`0` is valid) and encodes its content inside the
    /// JSON body, never into a header.
    #[tool(
        description = "Fetch the decoded content of one attachment by UID and positional `id` (`0` is valid). The content is returned base64-encoded inside the result."
    )]
    async fn get_attachment(
        &self,
        Parameters(input): Parameters<GetAttachmentInput>,
    ) -> Result<Json<service::AttachmentResponse>, String> {
        let (mailbox, uid, id) = input.into_parts()?;
        service::attachment(&self.state, mailbox, uid, id)
            .await
            .map(Json)
            .map_err(|error| tool_error(&error))
    }

    /// Reports the state of the IDLE subscription.
    ///
    /// Thin wrapper over [`service::idle_status`], which is infallible: the
    /// snapshot is a read of the supervisor's own task handles, with no network
    /// and no credential. Starting and stopping the subscription is deliberately
    /// not part of this surface.
    #[tool(
        description = "Report the IDLE subscription's state: `running` or `stopped`, the watched mailbox and its last error code (`null` when the last operation succeeded). Starting and stopping it is not exposed over MCP."
    )]
    async fn idle_status(&self) -> Json<crate::idle::IdleStatus> {
        Json(service::idle_status(&self.state).await)
    }

    /// Reports the delivery queue's observable state.
    ///
    /// Thin wrapper over [`service::idle_queue`], which is infallible and takes
    /// its snapshot without I/O even when the queue is backed by a file.
    #[tool(
        description = "Report the webhook delivery queue: whether it is persistent, how many notifications are pending, delivered, dropped and failed, and the age in seconds of the oldest pending one (`null` when none is pending)."
    )]
    async fn idle_queue(&self) -> Json<crate::queue::QueueStats> {
        Json(service::idle_queue(&self.state))
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
                 connection is authenticated, then `get_account`, `list_mailboxes`, \
                 `list_messages` and the message tools to read the account. Every failure \
                 is reported as a tool error carrying the stable code of the operation \
                 (`imap_unavailable`, `mailbox_not_found`, …).",
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
