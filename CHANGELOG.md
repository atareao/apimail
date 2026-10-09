# Changelog
## [0.2.0] - 2026-10-09

### Bug Fixes

- *(smtp)* Keep third-party SMTP error text out of the 502 body
- *(logs)* Keep startup error logs free of values and third-party text

### Documentation

- Add PLAN.md roadmap for the IMAP/SMTP CRUD (#5)
- *(openspec)* Add api-auth change proposal
- Document APIMAIL_API_KEY and authentication in README (#7)
- *(openspec)* Add and archive the mail-account change
- Document the mail account configuration and GET /api/account
- *(openspec)* Add and archive the smtp-send change
- Document POST /api/messages and APIMAIL_MAX_ATTACHMENT_BYTES
- *(openspec)* Add and archive the imap-connection change
- Document GET /api/imap/status and APIMAIL_IMAP_TIMEOUT_SECS
- *(openspec)* Add and archive the imap-mailboxes change
- Document GET /api/mailboxes and POST /api/mailboxes/select
- *(openspec)* Add and archive the imap-messages change
- Document GET /api/messages and GET /api/messages/{uid}
- *(openspec)* Add and archive the smtp-error-hygiene change
- *(openspec)* Add and archive the imap-flags change
- Document the flag, move, copy and delete endpoints
- *(openspec)* Add and archive the mime-parsing change
- Document the body and attachment endpoints
- *(openspec)* Add and archive the imap-idle change
- Document the IDLE endpoints and webhook configuration
- *(openspec)* Add and archive the startup-log-hygiene change

### Features

- *(auth)* Protect the API with a static Bearer API key
- *(mail)* Configure the IMAP/SMTP account from the environment
- *(smtp)* Send outgoing mail through the configured SMTP account
- *(imap)* Add lazy IMAP connection manager and GET /api/imap/status
- *(mailboxes)* List and select mailboxes over the IMAP session
- *(messages)* Search and fetch messages over the IMAP session
- *(flags)* Update flags, move, copy and delete messages over IMAP
- *(mime)* Parse message text, HTML and attachments over IMAP
- *(idle)* Watch the mailbox over IDLE and notify a webhook
## [0.1.0] - 2026-10-07

### Features

- Axum HTTP skeleton with /api/health (+ SDD setup & gitflow CI) (#1)

### Miscellaneous Tasks

- Initial commit
- Add crates.io publish metadata (description, MIT license, README) (#3)
- Release v0.1.0
