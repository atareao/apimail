# Spec Delta

## Purpose

Mantiene el arranque configurable del servidor HTTP, ahora condicionado a que la
autenticación **y** la cuenta de correo estén configuradas.

## MODIFIED Requirements

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
