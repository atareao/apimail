# Spec Delta

## MODIFIED Requirements

### Requirement: Port validation
The service SHALL validate an explicitly configured port and SHALL refuse to start when it is not a number between 1 and 65535. A value that is empty or whitespace-only counts as unset: the port then falls back to the value derived from the TLS mode.

#### Scenario: Invalid port
- **WHEN** `APIMAIL_IMAP_PORT` is set to `abc` or `0`
- **THEN** startup fails with a non-zero exit and a descriptive error

#### Scenario: Blank port counts as unset
- **WHEN** `APIMAIL_IMAP_PORT` is set to an empty or whitespace-only value
- **THEN** the IMAP port falls back to the value derived from the TLS mode

### Requirement: Default ports
The service SHALL derive the default port from the configured TLS mode when the port variable is unset or blank: IMAP uses `993` (implicit), `143` (starttls) or `143` (none); SMTP uses `465` (implicit), `587` (starttls) or `587` (none).

#### Scenario: IMAP implicit default port
- **WHEN** `APIMAIL_IMAP_TLS` is unset or `implicit` and `APIMAIL_IMAP_PORT` is unset
- **THEN** the IMAP port is `993`

#### Scenario: SMTP starttls default port
- **WHEN** `APIMAIL_SMTP_TLS` is `starttls` and `APIMAIL_SMTP_PORT` is unset
- **THEN** the SMTP port is `587`

#### Scenario: Explicit port overrides the default
- **WHEN** `APIMAIL_IMAP_PORT` is set to a valid value
- **THEN** that value is used regardless of the TLS mode
