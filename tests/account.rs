//! Integration tests for the account information endpoint (`GET /api/account`).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-account-key";
const IMAP_HOST: &str = "imap.test.example";
const SMTP_HOST: &str = "smtp.test.example";
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-password";
const SMTP_USER: &str = "smtp-user@test.example";
const SMTP_PASSWORD: &str = "smtp-password";

/// Loads a [`Config`] with a known API key and a known mail account without
/// touching the process environment.
fn config() -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some(IMAP_HOST.to_string()),
        "APIMAIL_IMAP_PORT" => Some("1993".to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_SMTP_HOST" => Some(SMTP_HOST.to_string()),
        "APIMAIL_SMTP_PORT" => Some("1587".to_string()),
        "APIMAIL_SMTP_USER" => Some(SMTP_USER.to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some(SMTP_PASSWORD.to_string()),
        _ => None,
    })
    .expect("valid test config")
}

/// Builds the router with a known API key and mail account.
fn app() -> Router {
    build_router(AppState::from_config(&config()).expect("valid app state"))
}

/// Sends a `GET /api/account` request with an optional `Authorization` header.
async fn account(authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().uri("/api/account");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app()
        .oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond")
}

#[tokio::test]
async fn account_with_valid_key_returns_hosts_and_ports() {
    let response = account(Some(&format!("Bearer {API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body should be readable")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");

    assert_eq!(json["imap"]["host"], IMAP_HOST);
    assert_eq!(json["imap"]["port"], 1993);
    assert_eq!(json["smtp"]["host"], SMTP_HOST);
    assert_eq!(json["smtp"]["port"], 1587);

    let text = String::from_utf8_lossy(&body);
    for secret in [IMAP_USER, IMAP_PASSWORD, SMTP_USER, SMTP_PASSWORD] {
        assert!(
            !text.contains(secret),
            "account response leaked `{secret}`: {text}"
        );
    }
}

#[tokio::test]
async fn account_without_header_returns_401() {
    let response = account(None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn account_with_wrong_key_returns_401() {
    let response = account(Some("Bearer not-the-key")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
