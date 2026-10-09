# Changelog
## [0.3.1] - 2026-10-09

### Bug Fixes

- *(docs)* Clear the 19 rustdoc warnings and guard them in CI

### Documentation

- Mark PLAN-001 item 3 as released in v0.3.0
- *(openspec)* Add rustdoc-hygiene change proposal
- *(openspec)* Archive rustdoc-hygiene (no spec deltas)
- Stop calling apimail a work in progress
## [0.3.0] - 2026-10-09

### Documentation

- Mark PLAN-001 items 1 and 2 as done in v0.2.1
- *(openspec)* Add and archive the webhook-delivery change
- Mark PLAN-001 item 3 as done

### Features

- *(idle)* Deliver webhook notifications through a durable queue

### Miscellaneous Tasks

- Release v0.3.0
## [0.2.1] - 2026-10-09

### Bug Fixes

- *(release)* Sync Cargo.lock on bump and keep process artifacts out of the crate

### Documentation

- Move the roadmap into plans/ and add PLAN-001.md
- *(openspec)* Add and archive the release-hygiene change

### Miscellaneous Tasks

- Release v0.2.1
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

### Miscellaneous Tasks

- Release v0.2.0
## [0.1.0] - 2026-10-07

### Features

- Axum HTTP skeleton with /api/health (+ SDD setup & gitflow CI) (#1)

### Miscellaneous Tasks

- Initial commit
- Add crates.io publish metadata (description, MIT license, README) (#3)
- Release v0.1.0
