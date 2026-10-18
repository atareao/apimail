# Spec Delta

## Purpose

Hacer que el arranque del servidor sea consciente del modo MCP stdio: cuando el transporte stdio
está activo, el servicio sirve MCP por stdin/stdout en lugar de enlazar un puerto TCP, sin cambiar
el arranque HTTP por defecto ni ningún otro requisito de `http-api`.

## MODIFIED Requirements

### Requirement: Server startup and configuration
The service SHALL start an HTTP server bound to a configurable host and port, and SHALL validate its required configuration — including the API key and the mail account credentials — at startup. When the MCP stdio transport is active, the service SHALL instead serve MCP over its standard input and standard output and SHALL NOT bind any TCP port.

#### Scenario: Start with defaults
- **WHEN** the service starts with a valid `APIMAIL_API_KEY` and a complete mail account configuration and without `APIMAIL_HOST` or `APIMAIL_PORT`
- **THEN** it binds to host `0.0.0.0` and port `3000`

#### Scenario: Start with environment overrides
- **WHEN** `APIMAIL_HOST` and `APIMAIL_PORT` are set to valid values
- **THEN** the service binds to the provided host and port

#### Scenario: Invalid port
- **WHEN** `APIMAIL_PORT` is not a valid `u16`
- **THEN** startup fails with a non-zero exit and a descriptive error instead of falling back to the default

#### Scenario: stdio mode serves MCP instead of HTTP
- **WHEN** `APIMAIL_MCP_STDIO` is true
- **THEN** no TCP port is bound
- **AND** MCP is served over standard input and standard output
