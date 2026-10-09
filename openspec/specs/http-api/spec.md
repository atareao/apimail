# http-api Specification

## Purpose
Expone un servidor HTTP Axum con configuración de arranque y un endpoint de salud que permite verificar que el servicio está vivo y desplegado correctamente.

## Requirements

### Requirement: Server startup and configuration
The service SHALL start an HTTP server bound to a configurable host and port, and SHALL validate its required configuration — including the API key and the mail account credentials — at startup.

#### Scenario: Start with defaults
- **WHEN** the service starts with a valid `APIMAIL_API_KEY` and a complete mail account configuration and without `APIMAIL_HOST` or `APIMAIL_PORT`
- **THEN** it binds to host `0.0.0.0` and port `3000`

#### Scenario: Start with environment overrides
- **WHEN** `APIMAIL_HOST` and `APIMAIL_PORT` are set to valid values
- **THEN** the service binds to the provided host and port

#### Scenario: Invalid port
- **WHEN** `APIMAIL_PORT` is not a valid `u16`
- **THEN** startup fails with a non-zero exit and a descriptive error instead of falling back to the default

### Requirement: Health endpoint
The service SHALL expose `GET /api/health` returning HTTP `200` with an `application/json` body.

#### Scenario: Healthy response
- **WHEN** a client sends `GET /api/health`
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body contains `"status":"ok"` along with the application name and version

#### Scenario: Unknown route
- **WHEN** a client requests a route that does not exist, such as `GET /api/unknown`
- **THEN** the response status is `404`

#### Scenario: Wrong method
- **WHEN** a client sends a method the route does not handle, such as `POST /api/health`
- **THEN** the response status is `405`
