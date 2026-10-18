//! Binary entrypoint: loads configuration, initialises tracing and serves the
//! HTTP API.

use std::process::ExitCode;

use apimail::{AppState, Config, build_router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    // Ignore the error if a global subscriber was already installed; falling
    // back to whatever is registered is fine for a binary entrypoint.
    if let Err(error) = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
    {
        eprintln!("tracing subscriber already initialised: {error}");
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                kind = error.kind(),
                "failed to start apimail: {}",
                error.public_message()
            );
            return ExitCode::FAILURE;
        }
    };

    // MCP over stdio is a *different* server, not a second listener: it takes
    // over standard input/output and binds no TCP port, so the branch has to
    // happen before the listener is created. The HTTP path below is untouched.
    #[cfg(feature = "mcp")]
    if config.mcp_stdio {
        tracing::info!("apimail serving MCP over stdio");
        return match apimail::mcp::serve_stdio(&config).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(kind = error.kind(), "MCP stdio server failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let listener = match TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("failed to bind {}:{}: {error}", config.host, config.port);
            return ExitCode::FAILURE;
        }
    };

    let local_addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| format!("{}:{}", config.host, config.port));
    let state = match AppState::from_config(&config) {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(
                kind = error.kind(),
                "failed to initialise application services: {}",
                error.public_message()
            );
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("apimail listening on {local_addr}");

    if let Err(error) = axum::serve(listener, build_router(state)).await {
        tracing::error!("server error: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
