//! Durable, bounded delivery queue for webhook notifications.
//!
//! The queue decouples IDLE ingestion from webhook delivery: a new message is
//! enqueued (durably, when a path is configured) and a delivery worker drains it
//! in FIFO order. When no path is configured the queue lives in memory only and
//! nothing is ever written to disk.
//!
//! Persistence is an append-only JSONL log: one record per line, replayed in
//! order when the queue is reopened. `enqueue` appends a record and `fsync`s the
//! file before returning, so a notification acknowledged to the mail server is
//! never lost. The log is compacted (atomically, via a temporary file and a
//! `rename`) only when it grows well past its live payload, and a torn last line
//! is discarded on reload.
//!
//! See `openspec/changes/webhook-delivery/design.md` for the rationale.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::idle::WebhookPayload;
use crate::imap::SendFuture;

/// Floor for the compaction threshold (64 KiB).
const MIN_COMPACT_BYTES: u64 = 64 * 1024;

/// Limits of the queue.
#[derive(Debug, Clone, Copy)]
pub struct QueueLimits {
    /// Maximum number of pending notifications.
    pub max_items: usize,
    /// Maximum number of live bytes held by pending notifications.
    pub max_bytes: u64,
}

impl Default for QueueLimits {
    /// Production defaults, matching `APIMAIL_QUEUE_MAX_ITEMS` and
    /// `APIMAIL_QUEUE_MAX_BYTES`.
    fn default() -> Self {
        Self {
            max_items: crate::config::DEFAULT_QUEUE_MAX_ITEMS,
            max_bytes: crate::config::DEFAULT_QUEUE_MAX_BYTES,
        }
    }
}

/// One queued notification.
#[derive(Debug, Clone)]
pub struct QueuedNotification {
    /// Monotonic, persisted sequence number.
    pub seq: u64,
    /// The notification payload.
    pub payload: WebhookPayload,
    /// Enqueue time, in Unix milliseconds.
    pub enqueued_unix_ms: u64,
    /// Delivery attempts observed so far (in memory only).
    pub attempts: u32,
}

/// Mailbox watermark, persisted next to the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredWatermark {
    /// Mailbox the watermark refers to.
    pub mailbox: String,
    /// UID validity of the mailbox, when the server reported it.
    pub uid_validity: Option<u32>,
    /// Last UID already ingested from the mailbox.
    pub last_uid: u32,
}

/// Observable state of the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueStats {
    /// Whether the queue is backed by a file.
    pub persistent: bool,
    /// Number of pending notifications.
    pub pending: usize,
    /// Notifications delivered so far.
    pub delivered: u64,
    /// Notifications dropped because a limit was exceeded.
    pub dropped: u64,
    /// Notifications definitively discarded after bounded attempts.
    pub failed: u64,
    /// Age, in seconds, of the oldest pending notification.
    pub oldest_pending_secs: Option<u64>,
}

/// Errors produced by the queue.
///
/// The single variant carries no third-party text: a queue file must never leak
/// credentials or upstream error messages.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// The queue storage is unavailable.
    #[error("the queue storage is unavailable")]
    Storage,
}

/// Injectable wall clock, in Unix milliseconds.
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Reads the current time from an injected clock.
fn clock_now(clock: &Clock) -> u64 {
    (**clock)()
}

/// The wall clock used in production.
fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The default (system) clock.
fn default_clock() -> Clock {
    Arc::new(system_now_ms)
}

/// Locks the shared state, recovering from a poisoned mutex.
fn lock(inner: &Arc<Mutex<Inner>>) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs a blocking closure on the blocking pool.
async fn run_blocking<T, F>(task: F) -> Result<T, QueueError>
where
    F: FnOnce() -> Result<T, QueueError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|_| QueueError::Storage)?
}

/// A durable, bounded, FIFO delivery queue.
pub struct WebhookQueue {
    inner: Arc<Mutex<Inner>>,
    limits: QueueLimits,
}

/// Mutable queue state, shared behind a mutex.
struct Inner {
    persistence: Option<Persistence>,
    entries: VecDeque<Pending>,
    live_bytes: u64,
    next_seq: u64,
    delivered: u64,
    dropped: u64,
    failed: u64,
    watermark: Option<StoredWatermark>,
    clock: Clock,
}

/// A pending notification plus the size of its log line.
struct Pending {
    notification: QueuedNotification,
    bytes: u64,
}

/// On-disk backing of a persistent queue.
///
/// The file handle is not cached: every append reopens the path, so a queue
/// file that is removed, rotated or replaced is detected before anything is
/// written (and owner-only permissions are reaffirmed on each open).
struct Persistence {
    path: PathBuf,
    file_bytes: u64,
}

impl WebhookQueue {
    /// Creates an in-memory queue (nothing is ever written to disk).
    pub fn in_memory(limits: QueueLimits) -> Self {
        Self::in_memory_with_clock(limits, default_clock())
    }

    /// In-memory queue with an injected clock (tests).
    fn in_memory_with_clock(limits: QueueLimits, clock: Clock) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                persistence: None,
                entries: VecDeque::new(),
                live_bytes: 0,
                next_seq: 1,
                delivered: 0,
                dropped: 0,
                failed: 0,
                watermark: None,
                clock,
            })),
            limits,
        }
    }

    /// Creates or opens the persistent queue file (owner-only permissions) and
    /// reloads its state. Fails when the path is not usable.
    pub fn persistent(path: impl Into<PathBuf>, limits: QueueLimits) -> Result<Self, QueueError> {
        Self::persistent_with_clock(path, limits, default_clock())
    }

    /// Persistent queue with an injected clock (tests).
    fn persistent_with_clock(
        path: impl Into<PathBuf>,
        limits: QueueLimits,
        clock: Clock,
    ) -> Result<Self, QueueError> {
        let path = path.into();
        let loaded = load_state(&path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                persistence: Some(Persistence {
                    path,
                    file_bytes: loaded.file_bytes,
                }),
                entries: loaded.entries,
                live_bytes: loaded.live_bytes,
                next_seq: loaded.next_seq,
                delivered: loaded.delivered,
                dropped: loaded.dropped,
                failed: loaded.failed,
                watermark: loaded.watermark,
                clock,
            })),
            limits,
        })
    }

    /// Whether the queue is backed by a file.
    pub fn is_persistent(&self) -> bool {
        lock(&self.inner).persistence.is_some()
    }

    /// Enqueues a notification, dropping the oldest pending ones when a limit
    /// would be exceeded. When persistent, the record is `fsync`ed first.
    pub async fn enqueue(&self, payload: WebhookPayload) -> Result<(), QueueError> {
        let inner = Arc::clone(&self.inner);
        let limits = self.limits;
        run_blocking(move || lock(&inner).enqueue_blocking(payload, limits)).await
    }

    /// Returns the oldest pending notification without extracting it, bumping
    /// its in-memory attempt counter.
    pub async fn peek_oldest(&self) -> Result<Option<QueuedNotification>, QueueError> {
        Ok(lock(&self.inner).peek_oldest())
    }

    /// Confirms delivery: extracts the notification and counts it as delivered.
    pub async fn ack(&self, seq: u64) -> Result<(), QueueError> {
        let inner = Arc::clone(&self.inner);
        run_blocking(move || lock(&inner).ack_blocking(seq)).await
    }

    /// Definitively discards a poison notification and counts it as failed.
    pub async fn discard(&self, seq: u64) -> Result<(), QueueError> {
        let inner = Arc::clone(&self.inner);
        run_blocking(move || lock(&inner).discard_blocking(seq)).await
    }

    /// Snapshot of the observable queue state.
    pub fn stats(&self) -> QueueStats {
        let inner = lock(&self.inner);
        let now = clock_now(&inner.clock);
        let oldest_pending_secs = inner
            .entries
            .front()
            .map(|pending| now.saturating_sub(pending.notification.enqueued_unix_ms) / 1000);
        QueueStats {
            persistent: inner.persistence.is_some(),
            pending: inner.entries.len(),
            delivered: inner.delivered,
            dropped: inner.dropped,
            failed: inner.failed,
            oldest_pending_secs,
        }
    }

    /// The persisted mailbox watermark, if any.
    pub fn watermark(&self) -> Option<StoredWatermark> {
        lock(&self.inner).watermark.clone()
    }

    /// Persists the mailbox watermark.
    pub async fn set_watermark(&self, watermark: StoredWatermark) -> Result<(), QueueError> {
        let inner = Arc::clone(&self.inner);
        run_blocking(move || lock(&inner).set_watermark_blocking(watermark)).await
    }
}

/// Object-safe view of the delivery queue used by the IDLE supervisor and worker.
///
/// Both [`run_supervisor`](crate::idle) and
/// [`run_delivery_worker`](crate::idle) depend on this trait rather than on the
/// concrete [`WebhookQueue`], so a test can inject a queue that fails on demand
/// and a future implementation can replace the storage entirely. Every method
/// delegates to the matching [`WebhookQueue`] inherent method.
pub trait NotificationQueue: Send + Sync {
    /// Enqueues a notification, durably when the queue is persistent.
    fn enqueue<'a>(&'a self, payload: WebhookPayload) -> SendFuture<'a, Result<(), QueueError>>;

    /// Returns the oldest pending notification without extracting it.
    fn peek_oldest<'a>(&'a self) -> SendFuture<'a, Result<Option<QueuedNotification>, QueueError>>;

    /// Confirms delivery of the notification with `seq`.
    fn ack<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>>;

    /// Definitively discards the notification with `seq`.
    fn discard<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>>;

    /// Persists the mailbox watermark.
    fn set_watermark<'a>(
        &'a self,
        watermark: StoredWatermark,
    ) -> SendFuture<'a, Result<(), QueueError>>;

    /// Snapshot of the observable queue state.
    fn stats(&self) -> QueueStats;

    /// The persisted mailbox watermark, if any.
    fn watermark(&self) -> Option<StoredWatermark>;

    /// Whether the queue is backed by a file.
    fn is_persistent(&self) -> bool;
}

impl NotificationQueue for WebhookQueue {
    fn enqueue<'a>(&'a self, payload: WebhookPayload) -> SendFuture<'a, Result<(), QueueError>> {
        Box::pin(WebhookQueue::enqueue(self, payload))
    }

    fn peek_oldest<'a>(&'a self) -> SendFuture<'a, Result<Option<QueuedNotification>, QueueError>> {
        Box::pin(WebhookQueue::peek_oldest(self))
    }

    fn ack<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>> {
        Box::pin(WebhookQueue::ack(self, seq))
    }

    fn discard<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>> {
        Box::pin(WebhookQueue::discard(self, seq))
    }

    fn set_watermark<'a>(
        &'a self,
        watermark: StoredWatermark,
    ) -> SendFuture<'a, Result<(), QueueError>> {
        Box::pin(WebhookQueue::set_watermark(self, watermark))
    }

    fn stats(&self) -> QueueStats {
        WebhookQueue::stats(self)
    }

    fn watermark(&self) -> Option<StoredWatermark> {
        WebhookQueue::watermark(self)
    }

    fn is_persistent(&self) -> bool {
        WebhookQueue::is_persistent(self)
    }
}

impl Inner {
    /// Appends an enqueue record, evicting the oldest pending items as needed.
    ///
    /// The durable append happens **before** the in-memory model is touched: if
    /// it fails, there is no phantom item and `next_seq` does not advance. The
    /// overflow eviction runs only after a successful enqueue.
    fn enqueue_blocking(
        &mut self,
        payload: WebhookPayload,
        limits: QueueLimits,
    ) -> Result<(), QueueError> {
        let at = clock_now(&self.clock);
        let seq = self.next_seq;
        let line = enqueue_line(seq, at, &payload)?;
        let line_bytes = line.len() as u64 + 1;

        // A payload that cannot fit on its own is dropped, counted, and never
        // written: there is nothing to evict that would make room for it.
        if line_bytes > limits.max_bytes {
            self.dropped += 1;
            return Ok(());
        }

        // (1) Persist the record durably first. On failure the in-memory model
        // is left untouched: no item is added and `next_seq` stays put.
        self.persist_line(&line, true)?;

        // (2) Only now commit the enqueue to memory.
        self.next_seq = seq + 1;
        self.live_bytes = self.live_bytes.saturating_add(line_bytes);
        self.entries.push_back(Pending {
            notification: QueuedNotification {
                seq,
                payload,
                enqueued_unix_ms: at,
                attempts: 0,
            },
            bytes: line_bytes,
        });

        // (3) Then evict the oldest pending items to respect the limits.
        while (self.entries.len() as u64) > limits.max_items as u64
            || self.live_bytes > limits.max_bytes
        {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.live_bytes = self.live_bytes.saturating_sub(oldest.bytes);
            self.dropped += 1;
            self.persist_discard(oldest.notification.seq, "overflow")?;
        }

        self.maybe_compact()
    }

    /// Removes the oldest pending notification, bumping its attempt counter.
    fn peek_oldest(&mut self) -> Option<QueuedNotification> {
        let pending = self.entries.front_mut()?;
        pending.notification.attempts = pending.notification.attempts.saturating_add(1);
        Some(pending.notification.clone())
    }

    /// Extracts the notification and counts it as delivered.
    fn ack_blocking(&mut self, seq: u64) -> Result<(), QueueError> {
        if self.remove(seq) {
            self.delivered += 1;
        }
        let line = ack_line(seq)?;
        self.persist_line(&line, false)?;
        self.maybe_compact()
    }

    /// Extracts the notification and counts it as failed (poison).
    fn discard_blocking(&mut self, seq: u64) -> Result<(), QueueError> {
        if self.remove(seq) {
            self.failed += 1;
        }
        let line = discard_line(seq, "poison")?;
        self.persist_line(&line, false)?;
        self.maybe_compact()
    }

    /// Persists the watermark (durably: a resume point must not be lost).
    fn set_watermark_blocking(&mut self, watermark: StoredWatermark) -> Result<(), QueueError> {
        let line = watermark_line(&watermark)?;
        self.watermark = Some(watermark);
        self.persist_line(&line, true)?;
        self.maybe_compact()
    }

    /// Removes the pending notification with `seq`, if present.
    fn remove(&mut self, seq: u64) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|pending| pending.notification.seq == seq)
        else {
            return false;
        };
        if let Some(pending) = self.entries.remove(index) {
            self.live_bytes = self.live_bytes.saturating_sub(pending.bytes);
        }
        true
    }

    /// Appends one record to the log, optionally `fsync`ing it first.
    ///
    /// The file is reopened for every append: if the backing file is gone or
    /// cannot be reopened, the write fails instead of silently landing on a
    /// stale descriptor. `file_bytes` only advances once the bytes are written.
    fn persist_line(&mut self, line: &str, durable: bool) -> Result<(), QueueError> {
        let Some(persistence) = self.persistence.as_mut() else {
            return Ok(());
        };
        let mut file = open_append_0600(&persistence.path)?;
        file.write_all(line.as_bytes())
            .map_err(|_| QueueError::Storage)?;
        file.write_all(b"\n").map_err(|_| QueueError::Storage)?;
        if durable {
            file.sync_all().map_err(|_| QueueError::Storage)?;
        }
        persistence.file_bytes += line.len() as u64 + 1;
        Ok(())
    }

    /// Writes an overflow discard record (not durable: a lost record only means
    /// the notification is retried on the next start, never a lost delivery).
    fn persist_discard(&mut self, seq: u64, reason: &'static str) -> Result<(), QueueError> {
        let line = discard_line(seq, reason)?;
        self.persist_line(&line, false)
    }

    /// Compacts the log when it grows past the threshold.
    fn maybe_compact(&mut self) -> Result<(), QueueError> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Ok(());
        };
        let threshold = MIN_COMPACT_BYTES.max(self.live_bytes.saturating_mul(2));
        if persistence.file_bytes > threshold {
            self.compact()?;
        }
        Ok(())
    }

    /// Rewrites the log atomically: a snapshot line first, then one enqueue line
    /// per live item, through a temporary file and a `rename`.
    fn compact(&mut self) -> Result<(), QueueError> {
        let mut content = String::new();
        let snapshot = LogRecord::Snapshot(SnapshotEntry {
            next_seq: self.next_seq,
            delivered: self.delivered,
            dropped: self.dropped,
            failed: self.failed,
            watermark: self.watermark.clone(),
        });
        content.push_str(&serde_json::to_string(&snapshot).map_err(|_| QueueError::Storage)?);
        content.push('\n');
        for pending in &self.entries {
            let notification = &pending.notification;
            content.push_str(&enqueue_line(
                notification.seq,
                notification.enqueued_unix_ms,
                &notification.payload,
            )?);
            content.push('\n');
        }
        let new_bytes = content.len() as u64;

        let persistence = self.persistence.as_mut().ok_or(QueueError::Storage)?;
        let temp = temp_path(&persistence.path);
        write_atomic(&temp, &persistence.path, content.as_bytes())?;
        // `write_atomic` creates the temporary file owner-only and renames it
        // into place; reaffirm the mode explicitly after the rename.
        harden_permissions(&persistence.path)?;
        persistence.file_bytes = new_bytes;
        Ok(())
    }
}

/// State reconstructed from a queue file at startup.
struct LoadedState {
    entries: VecDeque<Pending>,
    live_bytes: u64,
    next_seq: u64,
    delivered: u64,
    dropped: u64,
    failed: u64,
    watermark: Option<StoredWatermark>,
    file_bytes: u64,
}

impl LoadedState {
    /// Replays one log record.
    fn apply(&mut self, record: LogRecord, line_bytes: u64) {
        match record {
            LogRecord::Snapshot(snapshot) => {
                self.next_seq = snapshot.next_seq;
                self.delivered = snapshot.delivered;
                self.dropped = snapshot.dropped;
                self.failed = snapshot.failed;
                self.watermark = snapshot.watermark;
                self.entries.clear();
                self.live_bytes = 0;
            }
            LogRecord::Enqueue(entry) => {
                self.entries.push_back(Pending {
                    notification: QueuedNotification {
                        seq: entry.seq,
                        payload: entry.payload,
                        enqueued_unix_ms: entry.at,
                        attempts: 0,
                    },
                    bytes: line_bytes,
                });
                self.live_bytes = self.live_bytes.saturating_add(line_bytes);
                self.next_seq = self.next_seq.max(entry.seq.saturating_add(1));
            }
            LogRecord::Ack(entry) => {
                if self.extract(entry.seq) {
                    self.delivered += 1;
                }
            }
            LogRecord::Discard(entry) => {
                if self.extract(entry.seq) {
                    match entry.reason.as_str() {
                        "overflow" => self.dropped += 1,
                        _ => self.failed += 1,
                    }
                }
            }
            LogRecord::Watermark(watermark) => self.watermark = Some(watermark),
        }
    }

    /// Removes a pending entry by sequence number, returning whether it existed.
    fn extract(&mut self, seq: u64) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|pending| pending.notification.seq == seq)
        else {
            return false;
        };
        if let Some(pending) = self.entries.remove(index) {
            self.live_bytes = self.live_bytes.saturating_sub(pending.bytes);
        }
        true
    }
}

/// Opens (creating if needed) the queue file for appending, owner-only.
fn load_state(path: &Path) -> Result<LoadedState, QueueError> {
    // Creating/opening here is the usability check; a bad path fails closed.
    let probe = open_append_0600(path)?;
    let data = std::fs::read(path).map_err(|_| QueueError::Storage)?;

    let mut state = LoadedState {
        entries: VecDeque::new(),
        live_bytes: 0,
        next_seq: 1,
        delivered: 0,
        dropped: 0,
        failed: 0,
        watermark: None,
        file_bytes: 0,
    };

    let mut valid_end = 0usize;
    let mut offset = 0usize;
    while offset < data.len() {
        // A trailing partial line (no newline, or invalid JSON) ends the log.
        let Some(newline) = data[offset..].iter().position(|&byte| byte == b'\n') else {
            break;
        };
        let end = offset + newline;
        let record = std::str::from_utf8(&data[offset..end])
            .ok()
            .and_then(|text| serde_json::from_str::<LogRecord>(text).ok());
        let Some(record) = record else {
            break;
        };
        state.apply(record, (newline + 1) as u64);
        offset = end + 1;
        valid_end = offset;
    }

    // Drop a torn trailing line so future appends stay valid.
    if valid_end < data.len() {
        probe
            .set_len(valid_end as u64)
            .map_err(|_| QueueError::Storage)?;
    }
    state.file_bytes = valid_end as u64;
    Ok(state)
}

/// Writes `bytes` to `target` atomically: a temporary file plus a `rename`.
fn write_atomic(temp: &Path, target: &Path, bytes: &[u8]) -> Result<(), QueueError> {
    {
        let mut file = open_new_0600(temp)?;
        file.write_all(bytes).map_err(|_| QueueError::Storage)?;
        file.sync_all().map_err(|_| QueueError::Storage)?;
    }
    std::fs::rename(temp, target).map_err(|_| QueueError::Storage)
}

/// A sibling temporary path for `target`, unique within the process.
fn temp_path(target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = target
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from("queue"));
    name.push(format!(".{}.{unique}.tmp", std::process::id()));
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// Reaffirms owner-only permissions on a queue file (no-op off Unix).
#[cfg(unix)]
fn harden_permissions(path: &Path) -> Result<(), QueueError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| QueueError::Storage)
}

/// Reaffirms owner-only permissions on a queue file (no-op off Unix).
#[cfg(not(unix))]
fn harden_permissions(_path: &Path) -> Result<(), QueueError> {
    Ok(())
}

/// Opens (creating if needed) a file for appending with owner-only permissions.
///
/// `mode` only applies at creation, so an already-existing file is tightened to
/// `0600` as well.
#[cfg(unix)]
fn open_append_0600(path: &Path) -> Result<File, QueueError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| QueueError::Storage)?;
    harden_permissions(path)?;
    Ok(file)
}

/// Opens (creating if needed) a file for appending.
#[cfg(not(unix))]
fn open_append_0600(path: &Path) -> Result<File, QueueError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| QueueError::Storage)
}

/// Creates (truncating) a file with owner-only permissions.
#[cfg(unix)]
fn open_new_0600(path: &Path) -> Result<File, QueueError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| QueueError::Storage)
}

/// Creates (truncating) a file.
#[cfg(not(unix))]
fn open_new_0600(path: &Path) -> Result<File, QueueError> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|_| QueueError::Storage)
}

/// One record of the append-only queue log.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LogRecord {
    /// A notification was enqueued.
    Enqueue(Box<EnqueueEntry>),
    /// A notification was delivered.
    Ack(AckEntry),
    /// A notification was discarded.
    Discard(DiscardEntry),
    /// The mailbox watermark advanced.
    Watermark(StoredWatermark),
    /// A compaction snapshot, always the first line after compaction.
    Snapshot(SnapshotEntry),
}

/// Enqueue record body.
#[derive(Serialize, Deserialize)]
struct EnqueueEntry {
    seq: u64,
    at: u64,
    payload: WebhookPayload,
}

/// Borrowed view of an enqueue record body, so logging never clones the payload.
#[derive(Serialize)]
struct EnqueueEntryRef<'a> {
    seq: u64,
    at: u64,
    payload: &'a WebhookPayload,
}

/// Borrowed view of an enqueue record: mirrors the `{"enqueue": {…}}` shape of
/// [`LogRecord`] without taking ownership of the payload.
#[derive(Serialize)]
struct EnqueueRecordRef<'a> {
    enqueue: EnqueueEntryRef<'a>,
}

/// Ack record body.
#[derive(Serialize, Deserialize)]
struct AckEntry {
    seq: u64,
}

/// Discard record body.
#[derive(Serialize, Deserialize)]
struct DiscardEntry {
    seq: u64,
    reason: String,
}

/// Compaction snapshot body.
#[derive(Serialize, Deserialize)]
struct SnapshotEntry {
    next_seq: u64,
    delivered: u64,
    dropped: u64,
    failed: u64,
    watermark: Option<StoredWatermark>,
}

/// Serialises an enqueue record, borrowing the payload instead of cloning it.
fn enqueue_line(seq: u64, at: u64, payload: &WebhookPayload) -> Result<String, QueueError> {
    let record = EnqueueRecordRef {
        enqueue: EnqueueEntryRef { seq, at, payload },
    };
    serde_json::to_string(&record).map_err(|_| QueueError::Storage)
}

/// Serialises an ack record.
fn ack_line(seq: u64) -> Result<String, QueueError> {
    serde_json::to_string(&LogRecord::Ack(AckEntry { seq })).map_err(|_| QueueError::Storage)
}

/// Serialises a discard record.
fn discard_line(seq: u64, reason: &'static str) -> Result<String, QueueError> {
    let record = LogRecord::Discard(DiscardEntry {
        seq,
        reason: reason.to_string(),
    });
    serde_json::to_string(&record).map_err(|_| QueueError::Storage)
}

/// Serialises a watermark record.
fn watermark_line(watermark: &StoredWatermark) -> Result<String, QueueError> {
    serde_json::to_string(&LogRecord::Watermark(watermark.clone())).map_err(|_| QueueError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idle::{AddressPayload, AttachmentPayload, EnvelopePayload};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// A self-cleaning temporary directory (no external crate needed).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "apimail-queue-test-{tag}-{}-{}",
                std::process::id(),
                TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_clock(initial_ms: u64) -> (Arc<AtomicU64>, Clock) {
        let handle = Arc::new(AtomicU64::new(initial_ms));
        let clock: Clock = {
            let handle = Arc::clone(&handle);
            Arc::new(move || handle.load(Ordering::SeqCst))
        };
        (handle, clock)
    }

    fn limits(max_items: usize, max_bytes: u64) -> QueueLimits {
        QueueLimits {
            max_items,
            max_bytes,
        }
    }

    /// A rich payload exercising every nested field (round-trip sensitive).
    fn payload(uid: u32) -> WebhookPayload {
        WebhookPayload {
            mailbox: "INBOX".to_string(),
            uid,
            uid_validity: Some(7),
            flags: vec!["\\Seen".to_string()],
            size: Some(123),
            internal_date: Some("2024-01-02T03:04:05Z".to_string()),
            envelope: Some(EnvelopePayload {
                from: vec![AddressPayload {
                    name: Some("Alice".to_string()),
                    address: Some("alice@example.com".to_string()),
                }],
                to: vec![AddressPayload {
                    name: None,
                    address: Some("bob@example.com".to_string()),
                }],
                cc: Vec::new(),
                subject: Some("Hello".to_string()),
                date: Some("Tue, 2 Jan 2024 03:04:05 +0000".to_string()),
                message_id: Some("<id@example.com>".to_string()),
            }),
            parsed: true,
            text: Some(format!("body {uid}")),
            html: None,
            attachments: vec![AttachmentPayload {
                id: 0,
                filename: Some("a.txt".to_string()),
                content_type: Some("text/plain".to_string()),
                size: 5,
                inline: false,
                content_id: None,
            }],
        }
    }

    fn big_payload(uid: u32, text_bytes: usize) -> WebhookPayload {
        let mut p = payload(uid);
        p.text = Some("x".repeat(text_bytes));
        p.envelope = None;
        p.attachments = Vec::new();
        p
    }

    fn file_len(path: &std::path::Path) -> u64 {
        std::fs::metadata(path).expect("metadata").len()
    }

    fn first_line(path: &std::path::Path) -> String {
        let data = std::fs::read_to_string(path).expect("read");
        data.lines().next().unwrap_or_default().to_string()
    }

    fn wm() -> StoredWatermark {
        StoredWatermark {
            mailbox: "INBOX".to_string(),
            uid_validity: Some(7),
            last_uid: 42,
        }
    }

    #[tokio::test]
    async fn fifo_preserves_enqueue_order() {
        let (_h, clock) = fake_clock(1_000_000);
        let q = WebhookQueue::in_memory_with_clock(limits(100, u64::MAX), clock);
        for uid in 1..=3 {
            q.enqueue(payload(uid)).await.expect("enqueue");
        }
        for uid in 1..=3 {
            let n = q.peek_oldest().await.expect("peek").expect("some");
            assert_eq!(n.payload.uid, uid);
            q.ack(n.seq).await.expect("ack");
        }
        assert!(q.peek_oldest().await.expect("peek").is_none());
    }

    #[tokio::test]
    async fn enqueue_peek_ack_and_stats() {
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::in_memory_with_clock(limits(100, u64::MAX), clock);
        q.enqueue(payload(1)).await.expect("enqueue");
        q.enqueue(payload(2)).await.expect("enqueue");

        let s = q.stats();
        assert_eq!(s.pending, 2);
        assert_eq!(s.delivered, 0);
        assert_eq!(s.dropped, 0);
        assert_eq!(s.failed, 0);
        assert!(!s.persistent);

        let first = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(first.seq, 1);
        assert_eq!(first.attempts, 1);
        // Peeking does not extract.
        assert_eq!(q.stats().pending, 2);
        // The attempt counter keeps rising in memory.
        let second = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(second.seq, 1);
        assert_eq!(second.attempts, 2);

        q.ack(first.seq).await.expect("ack");
        let s = q.stats();
        assert_eq!(s.pending, 1);
        assert_eq!(s.delivered, 1);

        let remaining = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(remaining.seq, 2);
        assert_eq!(remaining.attempts, 1);
    }

    #[tokio::test]
    async fn discard_counts_as_failed() {
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::in_memory_with_clock(limits(100, u64::MAX), clock);
        q.enqueue(payload(1)).await.expect("enqueue");
        let n = q.peek_oldest().await.expect("peek").expect("some");
        q.discard(n.seq).await.expect("discard");
        let s = q.stats();
        assert_eq!(s.pending, 0);
        assert_eq!(s.failed, 1);
        assert_eq!(s.delivered, 0);
        assert_eq!(s.dropped, 0);
    }

    #[tokio::test]
    async fn oldest_pending_secs_uses_the_clock() {
        let (handle, clock) = fake_clock(1_000_000);
        let q = WebhookQueue::in_memory_with_clock(limits(100, u64::MAX), clock);
        assert_eq!(q.stats().oldest_pending_secs, None);

        q.enqueue(payload(1)).await.expect("enqueue");
        assert_eq!(q.stats().oldest_pending_secs, Some(0));

        handle.store(1_005_000, Ordering::SeqCst);
        assert_eq!(q.stats().oldest_pending_secs, Some(5));
    }

    #[test]
    fn in_memory_is_not_persistent() {
        let q = WebhookQueue::in_memory(limits(10, 1024));
        assert!(!q.stats().persistent);
        assert!(!q.is_persistent());
        assert!(q.watermark().is_none());
    }

    #[tokio::test]
    async fn persistent_reload_reconstructs_pending_counters_and_watermark() {
        let dir = TempDir::new("reload");
        let path = dir.path().join("queue.jsonl");
        let (_h, clock) = fake_clock(1_000_000);

        let q = WebhookQueue::persistent_with_clock(path.clone(), limits(100, u64::MAX), clock)
            .expect("persistent");
        assert!(q.is_persistent());
        q.enqueue(payload(10)).await.expect("enqueue"); // seq 1
        q.enqueue(payload(11)).await.expect("enqueue"); // seq 2
        q.enqueue(payload(12)).await.expect("enqueue"); // seq 3
        q.ack(1).await.expect("ack");
        q.discard(2).await.expect("discard");
        q.set_watermark(wm()).await.expect("watermark");
        let before = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(before.seq, 3);
        assert_eq!(before.attempts, 1);
        drop(q);

        let (_h2, clock2) = fake_clock(1_000_000);
        let q2 = WebhookQueue::persistent_with_clock(path, limits(100, u64::MAX), clock2)
            .expect("reload");

        let s = q2.stats();
        assert!(s.persistent);
        assert_eq!(s.pending, 1);
        assert_eq!(s.delivered, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.dropped, 0);
        assert_eq!(q2.watermark(), Some(wm()));

        let n = q2.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(n.seq, 3);
        assert_eq!(
            n.attempts, 1,
            "attempt counters restart from zero after a reload"
        );
        assert_eq!(n.enqueued_unix_ms, 1_000_000);
        assert_eq!(
            n.payload,
            payload(12),
            "payload round-trips through the log"
        );
    }

    #[tokio::test]
    async fn truncated_last_line_is_ignored() {
        use std::io::Write as _;

        let dir = TempDir::new("truncated");
        let path = dir.path().join("queue.jsonl");
        let (_h, clock) = fake_clock(500);

        let q = WebhookQueue::persistent_with_clock(path.clone(), limits(100, u64::MAX), clock)
            .expect("persistent");
        q.enqueue(payload(1)).await.expect("enqueue");
        q.enqueue(payload(2)).await.expect("enqueue");
        q.set_watermark(wm()).await.expect("watermark");
        drop(q);

        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open");
            file.write_all(b"{\"enqueue\":{\"seq\":3,\"at\":")
                .expect("partial write");
            file.flush().expect("flush");
        }

        let (_h2, clock2) = fake_clock(500);
        let q2 = WebhookQueue::persistent_with_clock(path.clone(), limits(100, u64::MAX), clock2)
            .expect("reload despite truncation");

        assert_eq!(q2.stats().pending, 2);
        assert_eq!(q2.watermark(), Some(wm()));
        let data = std::fs::read_to_string(&path).expect("read");
        assert!(data.ends_with('\n'), "the partial line is truncated away");
        assert_eq!(data.matches("\"enqueue\"").count(), 2);
    }

    #[tokio::test]
    async fn compaction_rewrites_the_log_when_it_grows() {
        let dir = TempDir::new("compact");
        let path = dir.path().join("queue.jsonl");
        let (_h, clock) = fake_clock(0);

        let limits = limits(10_000, 1 << 40);
        let q =
            WebhookQueue::persistent_with_clock(path.clone(), limits, clock).expect("persistent");
        for uid in 0..200 {
            q.enqueue(big_payload(uid, 800)).await.expect("enqueue");
        }
        let before = file_len(&path);
        assert!(before > 64 * 1024, "log should have grown past the floor");

        // Shrink the live set so the live bytes fall well below the log size.
        for seq in 1..=200 {
            q.ack(seq).await.expect("ack");
        }
        q.enqueue(big_payload(1000, 800)).await.expect("enqueue");
        q.enqueue(big_payload(1001, 800)).await.expect("enqueue");

        let after = file_len(&path);
        assert!(after < before, "compaction should shrink the log");
        assert!(after < 64 * 1024, "log after compaction is small");
        assert!(
            first_line(&path).contains("\"snapshot\""),
            "a compacted log starts with a snapshot"
        );

        // Surviving items are still readable, and the queue keeps working.
        let (_h2, clock2) = fake_clock(0);
        let q2 = WebhookQueue::persistent_with_clock(path.clone(), limits, clock2).expect("reload");
        assert_eq!(q2.stats().pending, 2);
        let first = q2.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(first.seq, 201);
        assert_eq!(first.payload.uid, 1000);
        q2.enqueue(big_payload(1002, 800))
            .await
            .expect("enqueue after compaction");
        let (_h3, clock3) = fake_clock(0);
        let q3 = WebhookQueue::persistent_with_clock(path, limits, clock3).expect("reload");
        assert_eq!(q3.stats().pending, 3);
    }

    #[tokio::test]
    async fn overflow_by_items_drops_the_oldest() {
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::in_memory_with_clock(limits(2, u64::MAX), clock);
        for uid in 1..=4 {
            q.enqueue(payload(uid)).await.expect("enqueue");
        }
        let s = q.stats();
        assert_eq!(s.pending, 2);
        assert_eq!(s.dropped, 2);
        assert_eq!(q.peek_oldest().await.expect("peek").expect("some").seq, 3);
    }

    #[tokio::test]
    async fn overflow_by_bytes_drops_the_oldest() {
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::in_memory_with_clock(limits(10_000, 2_600), clock);
        for uid in 1..=4 {
            q.enqueue(big_payload(uid, 800)).await.expect("enqueue");
        }
        let s = q.stats();
        assert_eq!(
            s.pending as u64 + s.dropped,
            4,
            "every enqueue is either pending or dropped"
        );
        assert!(s.dropped >= 1, "some notifications must be dropped");
        assert!(s.pending <= 2, "the live set must fit the byte budget");
        let oldest = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(
            oldest.seq,
            s.dropped + 1,
            "drops always evict the lowest sequence first"
        );
    }

    #[tokio::test]
    async fn payload_larger_than_the_byte_limit_is_dropped() {
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::in_memory_with_clock(limits(100, 100), clock);
        q.enqueue(big_payload(1, 5000)).await.expect("enqueue");
        let s = q.stats();
        assert_eq!(s.pending, 0);
        assert_eq!(s.dropped, 1);
    }

    #[test]
    fn persistent_with_unusable_path_fails() {
        let dir = TempDir::new("unusable");

        let as_directory = WebhookQueue::persistent(dir.path().to_path_buf(), limits(10, 1024));
        assert!(as_directory.is_err(), "a directory is not a usable path");

        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").expect("write blocker");
        let parent_is_a_file =
            WebhookQueue::persistent(blocker.join("queue.jsonl"), limits(10, 1024));
        assert!(
            parent_is_a_file.is_err(),
            "a path whose parent is a file is not usable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new("perm");
        let path = dir.path().join("queue.jsonl");
        let _q = WebhookQueue::persistent(path.clone(), limits(10, 1024)).expect("persistent");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the queue file must be owner-only");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persistent_reaffirms_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        fn mode_of(path: &std::path::Path) -> u32 {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        }

        let dir = TempDir::new("perm-reaffirm");
        let path = dir.path().join("queue.jsonl");

        // A pre-existing, world-readable queue file must be tightened on open.
        std::fs::write(&path, b"").expect("pre-create queue file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("lax mode");

        let (_h, clock) = fake_clock(0);
        let limits = limits(10_000, 1 << 40);
        let q =
            WebhookQueue::persistent_with_clock(path.clone(), limits, clock).expect("persistent");
        assert_eq!(
            mode_of(&path),
            0o600,
            "an existing queue file must be tightened to owner-only"
        );

        // Force a compaction (grow the log, then shrink the live set) and confirm
        // the rewritten file is still owner-only.
        for uid in 0..200 {
            q.enqueue(big_payload(uid, 800)).await.expect("enqueue");
        }
        assert!(file_len(&path) > 64 * 1024, "log should exceed the floor");
        for seq in 1..=200 {
            q.ack(seq).await.expect("ack");
        }
        q.enqueue(big_payload(1000, 800)).await.expect("enqueue");
        q.enqueue(big_payload(1001, 800)).await.expect("enqueue");
        assert!(
            file_len(&path) < 64 * 1024,
            "compaction should rewrite the log"
        );
        assert_eq!(
            mode_of(&path),
            0o600,
            "a compacted queue file must stay owner-only"
        );
    }

    #[tokio::test]
    async fn enqueue_that_cannot_persist_leaves_memory_untouched() {
        let dir = TempDir::new("enqueue-fail");
        let path = dir.path().join("queue.jsonl");
        let (_h, clock) = fake_clock(0);
        let q = WebhookQueue::persistent_with_clock(path.clone(), limits(100, u64::MAX), clock)
            .expect("persistent");

        let before = q.stats();

        // Make the storage genuinely unavailable: remove the queue file and its
        // directory so reopening the file for append fails deterministically
        // (this works even when the test runs as root, unlike a permission trick).
        std::fs::remove_file(&path).expect("remove queue file");
        std::fs::remove_dir_all(dir.path()).expect("remove queue directory");

        let result = q.enqueue(payload(1)).await;
        assert!(
            result.is_err(),
            "an enqueue that cannot be persisted must fail"
        );

        let after = q.stats();
        assert_eq!(after.pending, 0, "no phantom item is left in memory");
        assert_eq!(after.delivered, before.delivered);
        assert_eq!(after.dropped, before.dropped);
        assert_eq!(after.failed, before.failed);

        // `next_seq` did not advance: once storage is back, the next enqueue
        // reuses the sequence the failed one would have used.
        std::fs::create_dir_all(dir.path()).expect("recreate directory");
        q.enqueue(payload(2)).await.expect("enqueue after recovery");
        let recovered = q.peek_oldest().await.expect("peek").expect("some");
        assert_eq!(
            recovered.seq, 1,
            "the failed enqueue must not advance the sequence"
        );
    }
}
