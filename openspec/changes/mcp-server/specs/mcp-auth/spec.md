# Spec Delta

## Purpose

Definir cómo se autentica el acceso al servidor MCP: el transporte HTTP reutiliza la API key ya
existente y el transporte stdio hereda el entorno del proceso, sin añadir un segundo mecanismo ni
filtrar credenciales en el tráfico MCP.

## ADDED Requirements

### Requirement: The HTTP transport reuses the existing API key

Every request to the MCP HTTP endpoint SHALL require the same `Authorization: Bearer
<APIMAIL_API_KEY>` credential as the rest of the API, enforced by the existing authentication
middleware, and the service SHALL NOT introduce a second authentication mechanism or advertise any
OAuth metadata.

#### Scenario: Missing credential is 401 with the existing envelope
- **WHEN** the MCP HTTP endpoint is requested without an `Authorization` header
- **THEN** the response status is `401`
- **AND** the response body is the existing JSON error envelope and a `WWW-Authenticate: Bearer` challenge is present

#### Scenario: Wrong credential is 401
- **WHEN** the MCP HTTP endpoint is requested with a `Bearer` key that is not the configured one
- **THEN** the response status is `401`

#### Scenario: Valid credential reaches the transport
- **WHEN** the MCP HTTP endpoint is requested with `Authorization: Bearer <APIMAIL_API_KEY>`
- **THEN** the request reaches the MCP transport

#### Scenario: No OAuth metadata is advertised
- **WHEN** an MCP client probes the service for OAuth discovery documents
- **THEN** the service advertises no OAuth metadata and only accepts the API key

### Requirement: The stdio transport does not weaken the trust boundary

The stdio transport SHALL NOT require an additional credential: it SHALL inherit the process
environment, and startup SHALL remain fail-closed, so the service still refuses to run without a
configured `APIMAIL_API_KEY`.

#### Scenario: stdio inherits the process environment
- **WHEN** the service runs in stdio mode
- **THEN** it uses the same `APIMAIL_API_KEY` and mail account already loaded from the process environment, with no extra credential

#### Scenario: Startup still fails closed in stdio mode
- **WHEN** the service is asked to run in stdio mode without a configured `APIMAIL_API_KEY`
- **THEN** startup fails with a non-zero exit and a configuration error naming the variable

### Requirement: Credentials never appear in MCP traffic

The service SHALL never place the API key or the mail account password in any MCP tool result, tool
error, resource or log line, and authentication failures SHALL be opaque.

#### Scenario: No key echo
- **WHEN** any tool result or tool error is returned
- **THEN** it contains neither the API key nor the mail account password

#### Scenario: Authentication failure is opaque
- **WHEN** an MCP request fails authentication
- **THEN** the response reveals only that the credential is missing or invalid, and not the configured value
