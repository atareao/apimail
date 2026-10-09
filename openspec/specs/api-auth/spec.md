# api-auth Specification

## Purpose
Protege la API con una **API key estática** presentada en la cabecera
`Authorization` mediante el esquema `Bearer`. Todas las rutas quedan protegidas
salvo `GET /api/health`, y el servicio se niega a arrancar sin una clave
configurada.

## Requirements

### Requirement: API key configuration
The service SHALL require a non-empty API key read from the `APIMAIL_API_KEY` environment variable and SHALL refuse to start when it is missing or empty.

#### Scenario: Valid API key
- **WHEN** `APIMAIL_API_KEY` is set to a non-empty value
- **THEN** the configuration loads successfully

#### Scenario: Missing API key
- **WHEN** the service starts without `APIMAIL_API_KEY`
- **THEN** startup fails with a non-zero exit and a descriptive error instead of running unprotected

#### Scenario: Empty API key
- **WHEN** `APIMAIL_API_KEY` is set to an empty string
- **THEN** startup fails with a non-zero exit and a descriptive error

### Requirement: Protected routes
The service SHALL reject requests to every route except `GET /api/health` unless they carry the valid API key in the `Authorization` header using the `Bearer` scheme.

#### Scenario: Request with the valid key
- **WHEN** a protected route such as `GET /api/whoami` is requested with `Authorization: Bearer <APIMAIL_API_KEY>`
- **THEN** the response status is `200`

#### Scenario: Request without credentials
- **WHEN** a protected route is requested without an `Authorization` header
- **THEN** the response status is `401`

#### Scenario: Request with a wrong key
- **WHEN** a protected route is requested with an `Authorization` header carrying a key that is not the configured one
- **THEN** the response status is `401`

#### Scenario: Malformed authorisation header
- **WHEN** a protected route is requested with an `Authorization` header that does not use the `Bearer` scheme
- **THEN** the response status is `401`

#### Scenario: Health endpoint is public
- **WHEN** `GET /api/health` is requested without an `Authorization` header
- **THEN** the response status is `200`

### Requirement: Unauthorized response
The service SHALL answer requests rejected for a missing or invalid API key with HTTP `401`, an `application/json` body and a `WWW-Authenticate` challenge.

#### Scenario: Unauthorized payload
- **WHEN** a request is rejected for a missing or invalid API key
- **THEN** the response status is `401`
- **AND** the `Content-Type` is `application/json`
- **AND** the body contains an error message

#### Scenario: WWW-Authenticate challenge
- **WHEN** a request is rejected for a missing or invalid API key
- **THEN** the response includes a `WWW-Authenticate` header with the `Bearer` scheme

### Requirement: Authenticated identity endpoint
The service SHALL expose `GET /api/whoami` as a protected route returning HTTP `200` with an `application/json` body that reports the caller as authenticated.

#### Scenario: Authenticated caller
- **WHEN** `GET /api/whoami` is requested with a valid API key
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body contains `"authenticated":true`
