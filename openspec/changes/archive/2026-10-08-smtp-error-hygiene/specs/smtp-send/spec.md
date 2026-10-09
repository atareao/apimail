# Spec Delta

## Purpose
Envía correo saliente a través del SMTP configurado. Expone `POST /api/messages`
(protegido), acepta un mensaje en JSON con adjuntos en base64, respeta el modo TLS
de `mail-account`, acota el tamaño de los adjuntos y traduce los fallos del
servidor SMTP a respuestas HTTP claras.

## MODIFIED Requirements

### Requirement: SMTP delivery and TLS
The service SHALL deliver the message through the SMTP endpoint configured in `mail-account`, honouring its TLS mode (`implicit`, `starttls` or `none`), and SHALL answer with HTTP `502` when the SMTP server rejects or fails the delivery. The `502` body SHALL carry a stable, server-independent message that never reflects text produced by the SMTP server.

#### Scenario: Delivery failure
- **WHEN** the SMTP server fails or rejects the message
- **THEN** the response status is `502`
- **AND** the body contains an error message
- **AND** the message is a stable, server-independent description that does not include the SMTP server's response text

### Requirement: SMTP secret protection
The service SHALL never emit the SMTP password in logs, debug output or HTTP responses, and SHALL never emit text produced by the SMTP server (its response text, banners, host names or addresses) in logs, debug output or HTTP responses.

#### Scenario: Redacted state
- **WHEN** the application state is formatted with `Debug`
- **THEN** the output contains none of the configured secrets

#### Scenario: Delivery error does not leak server text
- **WHEN** the SMTP server returns an error whose text contains a host name, an address or a server banner
- **THEN** the `502` response body does not contain that text
