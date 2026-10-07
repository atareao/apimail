//! Integration tests for API key authentication.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-secret-key";

/// Loads a [`Config`] with a known API key without touching the process
/// environment.
fn config() -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        _ => None,
    })
    .expect("valid test config")
}

/// Builds the router with a known API key, exercising the protected middleware.
fn app() -> Router {
    build_router(AppState::from_config(&config()))
}

/// Sends a `GET /api/whoami` request with an optional `Authorization` header.
async fn whoami(authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().uri("/api/whoami");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app()
        .oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond")
}

#[tokio::test]
async fn whoami_with_valid_key_returns_200() {
    let response = whoami(Some(&format!("Bearer {API_KEY}"))).await;
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
    assert_eq!(json["authenticated"], true);
}

#[tokio::test]
async fn whoami_without_header_returns_401() {
    let response = whoami(None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn whoami_with_wrong_key_returns_401() {
    let response = whoami(Some("Bearer not-the-key")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn whoami_with_non_bearer_scheme_returns_401() {
    let response = whoami(Some(&format!("Basic {API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_is_public_without_header() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn unauthorized_response_is_json_with_challenge() {
    let response = whoami(None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body should be readable")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
    assert_eq!(json["error"], "unauthorized");
    assert!(json["message"].is_string(), "message should be a string");
}

#[tokio::test]
async fn bearer_scheme_is_case_insensitive() {
    let response = whoami(Some(&format!("bearer {API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn bearer_scheme_accepts_mixed_case() {
    let response = whoami(Some(&format!("BeArEr {API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn whoami_with_multiple_spaces_returns_200() {
    let response = whoami(Some(&format!("Bearer  {API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn whoami_with_tab_separator_returns_200() {
    let response = whoami(Some(&format!("Bearer\t{API_KEY}"))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn whoami_with_empty_token_returns_401() {
    let response = whoami(Some("Bearer ")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn whoami_with_non_ascii_header_returns_401() {
    let mut builder = Request::builder().uri("/api/whoami");
    builder = builder.header(header::AUTHORIZATION, "Bearer clé-secrète");
    let response = app()
        .oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn app_state_debug_does_not_leak_api_key() {
    let debug = format!("{:?}", AppState::from_config(&config()));
    assert!(
        !debug.contains(API_KEY),
        "Debug output must not leak the API key: {debug}"
    );
}
