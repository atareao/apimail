# Spec Delta

## Purpose

Define y valida la cuenta de correo (IMAP y SMTP) a partir de variables de
entorno. El servicio no arranca sin una cuenta completa (*fail-closed*), protege
los secretos y expone la información no sensible en `GET /api/account`.

## ADDED Requirements

### Requirement: Mail account configuration
The service SHALL build the mail account —an IMAP endpoint and an SMTP endpoint— from environment variables and SHALL refuse to start when any required value is missing or empty (a whitespace-only value counts as empty).

The required values per endpoint are host, user and password:
`APIMAIL_IMAP_HOST`/`APIMAIL_IMAP_USER`/`APIMAIL_IMAP_PASSWORD` and
`APIMAIL_SMTP_HOST`/`APIMAIL_SMTP_USER`/`APIMAIL_SMTP_PASSWORD`.

#### Scenario: Complete configuration
- **WHEN** the IMAP and SMTP host, user and password variables are all set to non-empty values
- **THEN** the configuration loads successfully

#### Scenario: Missing IMAP host
- **WHEN** `APIMAIL_IMAP_HOST` is unset
- **THEN** startup fails with a non-zero exit and a descriptive error

#### Scenario: Missing SMTP user
- **WHEN** `APIMAIL_SMTP_USER` is unset
- **THEN** startup fails with a non-zero exit and a descriptive error

#### Scenario: Empty required value
- **WHEN** a required value such as `APIMAIL_IMAP_PASSWORD` is set to an empty string
- **THEN** startup fails with a non-zero exit and a descriptive error

#### Scenario: Blank required value
- **WHEN** a required value such as `APIMAIL_IMAP_HOST` contains only whitespace
- **THEN** startup fails with a non-zero exit and a descriptive error

### Requirement: TLS mode
The service SHALL accept the TLS modes `implicit`, `starttls` and `none` (case-insensitively), SHALL default to `implicit`, and SHALL reject any other value.

#### Scenario: Default TLS mode
- **WHEN** neither `APIMAIL_IMAP_TLS` nor `APIMAIL_SMTP_TLS` is set
- **THEN** both endpoints use the `implicit` mode

#### Scenario: Plaintext mode with warning
- **WHEN** `APIMAIL_IMAP_TLS` is set to `none`
- **THEN** the configuration loads and a warning about the disabled TLS is logged

#### Scenario: Invalid TLS mode
- **WHEN** `APIMAIL_IMAP_TLS` is set to an unsupported value such as `ssl`
- **THEN** startup fails with a non-zero exit and a descriptive error

### Requirement: Default ports
The service SHALL derive the default port from the configured TLS mode when the port variable is unset: IMAP uses `993` (implicit), `143` (starttls) or `143` (none); SMTP uses `465` (implicit), `587` (starttls) or `587` (none).

#### Scenario: IMAP implicit default port
- **WHEN** `APIMAIL_IMAP_TLS` is unset or `implicit` and `APIMAIL_IMAP_PORT` is unset
- **THEN** the IMAP port is `993`

#### Scenario: SMTP starttls default port
- **WHEN** `APIMAIL_SMTP_TLS` is `starttls` and `APIMAIL_SMTP_PORT` is unset
- **THEN** the SMTP port is `587`

#### Scenario: Explicit port overrides the default
- **WHEN** `APIMAIL_IMAP_PORT` is set to a valid value
- **THEN** that value is used regardless of the TLS mode

### Requirement: Port validation
The service SHALL validate an explicitly configured port and SHALL refuse to start when it is not a number between 1 and 65535.

#### Scenario: Invalid port
- **WHEN** `APIMAIL_IMAP_PORT` is set to `abc` or `0`
- **THEN** startup fails with a non-zero exit and a descriptive error

### Requirement: Secret protection
The service SHALL never emit the mail account passwords in logs or in debug output.

#### Scenario: Redacted debug output
- **WHEN** the loaded configuration is formatted with `Debug`
- **THEN** the output contains a redaction marker
- **AND** the output contains none of the configured passwords

### Requirement: Account information endpoint
The service SHALL expose `GET /api/account` as a protected route returning, as JSON, only the host and port of the IMAP and SMTP endpoints and neither the username nor the password.

#### Scenario: Authenticated request
- **WHEN** `GET /api/account` is requested with a valid API key
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports the IMAP and SMTP host and port
- **AND** the body contains neither the username nor the password

#### Scenario: Unauthenticated request
- **WHEN** `GET /api/account` is requested without a valid API key
- **THEN** the response status is `401`
