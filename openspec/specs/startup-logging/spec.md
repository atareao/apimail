# startup-logging Specification

## Purpose
Define la higiene de los mensajes de error de arranque: `AccountError`, `ConfigError` y
`AppStateError` deben poder describirse sin filtrar valores de configuración, credenciales ni
texto de terceros, y el entrypoint debe usar esa descripción en sus logs.

## Requirements

### Requirement: Stable public messages for configuration errors

`AccountError` and `ConfigError` SHALL expose `public_message()` returning a description that
names the affected environment variable when known but never its value, and `kind()` returning
a stable machine-filterable tag. Neither accessor SHALL embed credentials or third-party text.

#### Scenario: A variable with an invalid value is described without the value

- **WHEN** a `ConfigError` or `AccountError` variant that carries an offending raw value is
  asked for its `public_message()`
- **THEN** the message names the offending variable AND does not contain the offending raw
  value

#### Scenario: A missing variable is described by name

- **WHEN** an `AccountError::MissingValue` or `ConfigError::MissingApiKey` is asked for its
  `public_message()`
- **THEN** the message names the missing variable AND contains no value

#### Scenario: Configuration error kind is stable

- **WHEN** any `ConfigError` or `AccountError` variant is asked for its `kind()`
- **THEN** the returned tag is one of a fixed set of static strings

### Requirement: Application service errors delegate to the inner public message

`AppStateError` SHALL expose `public_message()` and `kind()` that delegate to the wrapped
`SmtpError`/`ImapError`, so that error text coming from `lettre` or `async-imap` never reaches
a caller through the new accessors.

#### Scenario: SMTP initialisation failure hides third-party text

- **WHEN** an `AppStateError::Smtp` wrapping a transport or delivery error is asked for its
  `public_message()`
- **THEN** the message equals the inner `SmtpError::public_message()` AND does not contain the
  wrapped third-party text (for example a server response)

#### Scenario: IMAP initialisation failure hides third-party text

- **WHEN** an `AppStateError::Imap` wrapping a login, TLS or protocol error is asked for its
  `public_message()`
- **THEN** the message equals the inner `ImapError::public_message()` AND does not contain the
  wrapped third-party text (for example a server response or a host name)

#### Scenario: Service error kind identifies the subsystem

- **WHEN** an `AppStateError` is asked for its `kind()`
- **THEN** the returned tag is `"smtp"` or `"imap"`

### Requirement: Startup logs never print raw domain errors

The binary entrypoint SHALL log configuration and application-service startup failures using
`kind()` and `public_message()` instead of the `Display` implementation.

#### Scenario: Configuration failure is logged without raw values

- **WHEN** the entrypoint fails to load the configuration
- **THEN** its error log line is built from `kind()` and `public_message()` AND stderr does not
  contain the offending raw value

#### Scenario: Configuration failure still names the variable

- **WHEN** the entrypoint fails because a required or numeric variable is invalid or missing
- **THEN** stderr still contains the offending variable name

#### Scenario: Service initialisation failure is logged without third-party text

- **WHEN** the entrypoint fails to initialise the application services
- **THEN** its error log line is built from `kind()` and `public_message()` AND it does not
  contain text produced by `lettre`, `async-imap`, rustls or the operating system
