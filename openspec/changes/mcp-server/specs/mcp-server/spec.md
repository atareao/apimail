# Spec Delta

## Purpose

Permitir que apimail arranque como servidor MCP **opcional** por stdio o por Streamable HTTP
anidado en el mismo listener `axum`, con una configuración **fail-closed** y dejando intacto el
comportamiento actual cuando la feature está apagada o no se configura ningún transporte.

## ADDED Requirements

### Requirement: MCP support is an opt-in build feature

The service SHALL expose MCP only when it is compiled with the optional Cargo feature `mcp`, which
SHALL NOT be enabled by default, so a default build neither compiles the MCP dependencies nor
mounts any MCP endpoint; when an MCP environment variable is defined while the service was
compiled without that feature, startup SHALL fail with a configuration error naming the offending
variable.

#### Scenario: Default build exposes no MCP endpoint
- **WHEN** a default build (without the `mcp` feature) serves a request for the configured MCP path
- **THEN** the response status is `404`
- **AND** no MCP dependency is compiled into the binary

#### Scenario: MCP variables without the feature
- **WHEN** the service is compiled without the `mcp` feature and an MCP environment variable such as `APIMAIL_MCP_HTTP` is set
- **THEN** startup fails with a non-zero exit and a configuration error naming the offending variable

#### Scenario: Feature build keeps the HTTP API unchanged
- **WHEN** the service is compiled with the `mcp` feature but no MCP environment variable is set
- **THEN** every existing `/api/...` route keeps its previous behaviour

### Requirement: Exactly one MCP transport mode runs at a time

The service SHALL run at most one MCP transport per process, selected by `APIMAIL_MCP_STDIO` or
`APIMAIL_MCP_HTTP`, and SHALL refuse to start when both are requested at once.

#### Scenario: stdio mode serves over stdin/stdout and binds no TCP port
- **WHEN** `APIMAIL_MCP_STDIO` is true and `APIMAIL_MCP_HTTP` is false
- **THEN** MCP is served over standard input and standard output
- **AND** no TCP listener is created

#### Scenario: http mode mounts on the same listener and opens no extra port
- **WHEN** `APIMAIL_MCP_HTTP` is true and `APIMAIL_MCP_STDIO` is false
- **THEN** the MCP endpoint is served by the existing HTTP listener on `APIMAIL_HOST` and `APIMAIL_PORT`
- **AND** no additional TCP port is opened

#### Scenario: both requested fails
- **WHEN** `APIMAIL_MCP_STDIO` and `APIMAIL_MCP_HTTP` are both true
- **THEN** startup fails with a non-zero exit and a configuration error naming both variables

#### Scenario: neither requested keeps the previous behaviour
- **WHEN** neither `APIMAIL_MCP_STDIO` nor `APIMAIL_MCP_HTTP` is set
- **THEN** the service starts as a plain HTTP API and mounts no MCP transport

### Requirement: The Streamable HTTP transport is mounted on the existing router

The service SHALL mount the MCP Streamable HTTP transport as a Tower service nested in the existing
`axum::Router`, under a configurable path that defaults to `/mcp`, sharing the listener and the
application state with the HTTP API and opening no second server.

#### Scenario: Mounted at the default path
- **WHEN** `APIMAIL_MCP_HTTP` is true and `APIMAIL_MCP_PATH` is unset
- **THEN** the MCP transport is reachable under the path `/mcp` on the existing listener

#### Scenario: Configurable path
- **WHEN** `APIMAIL_MCP_PATH` is set to a valid absolute path such as `/mcp/email`
- **THEN** the MCP transport is reachable under that path

#### Scenario: Invalid path fails
- **WHEN** `APIMAIL_MCP_PATH` is set to a value that is not an absolute path (does not start with `/`) or is exactly `/`
- **THEN** startup fails with a configuration error naming the variable

#### Scenario: Reserved path fails
- **WHEN** `APIMAIL_MCP_PATH` is set to a path inside the reserved `/api` prefix
- **THEN** startup fails with a configuration error naming the variable

#### Scenario: No extra port
- **WHEN** `APIMAIL_MCP_HTTP` is true
- **THEN** the process listens on exactly one TCP port, the one configured through `APIMAIL_HOST` and `APIMAIL_PORT`

### Requirement: The stdio transport writes only protocol data to stdout

When the stdio transport is active, the service SHALL use standard output exclusively for the MCP
protocol stream and SHALL emit every diagnostic, log line and error message on standard error, and
SHALL NOT create a TCP listener.

#### Scenario: Clean stdout
- **WHEN** the service runs in stdio mode and a JSON-RPC message is exchanged
- **THEN** standard output carries only well-formed MCP protocol data

#### Scenario: Diagnostics on stderr
- **WHEN** the service runs in stdio mode and emits a log line or a startup error
- **THEN** that line is written to standard error and never to standard output

#### Scenario: No TCP listener
- **WHEN** the service runs in stdio mode
- **THEN** no TCP port is bound

### Requirement: MCP configuration is validated at startup

The service SHALL read and validate the MCP configuration at startup in a fail-closed manner: it
SHALL parse the two transport flags as booleans and the path as an absolute path outside the
reserved `/api` prefix, and SHALL fail with a configuration error naming the offending variable
when any value is invalid.

#### Scenario: Boolean parsing rejects unknown values
- **WHEN** `APIMAIL_MCP_STDIO` or `APIMAIL_MCP_HTTP` is set to a value that is neither a case-insensitive `true`/`false` nor `1`/`0`
- **THEN** startup fails with a configuration error naming the offending variable

#### Scenario: Defaults
- **WHEN** no MCP variable is set
- **THEN** both transports default to `false` and the path defaults to `/mcp`

#### Scenario: Path validation
- **WHEN** `APIMAIL_MCP_PATH` is empty, relative, exactly `/`, or inside the reserved `/api` prefix
- **THEN** startup fails with a configuration error naming `APIMAIL_MCP_PATH`
