//! Resumable download queue.
//!
//! Downloads stream into `<dir>/<id>.part`, resume with HTTP `Range`, verify
//! size and MD5, then move to `<dir>/<id>/<file_name>`. At most
//! `max_concurrent` run at once. Every state change is reported through the
//! `on_change` hook so the app can persist the queue and refresh the UI.
//!
//! Pause and cancel take effect at once, even while a worker is stuck in a
//! network read: each run gets its own control token, and reports from a
//! worker whose token was replaced or stopped are ignored.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use kombini_core::{AppError, ErrorCode};
use md5::{Digest, Md5};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Hard cap on a single download.
pub const MAX_DOWNLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;
/// Waits between automatic retries of a failed transfer, in seconds. The
/// count starts over whenever an attempt made progress.
const BACKOFF_SECS: [u64; 6] = [2, 4, 8, 15, 30, 30];
/// Wait after the server rate limited us.
const RATE_LIMIT_WAIT_SECS: u64 = 60;
const EVENT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadSpec {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub title: String,
    pub expected_md5: Option<String>,
    pub expected_size: Option<u64>,
    /// Opaque data the app needs to install the result (GameBanana ids, ...).
    #[serde(default)]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Downloading | Self::Verifying)
    }
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadItem {
    pub spec: DownloadSpec,
    pub status: Status,
    pub received: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
    pub error: Option<AppError>,
    /// Final file once completed.
    pub path: Option<PathBuf>,
    pub created_at: i64,
}

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

struct Slot {
    item: DownloadItem,
    control: Arc<AtomicU8>,
}

type Listener = Arc<dyn Fn(&DownloadItem) + Send + Sync>;

struct Inner {
    dir: PathBuf,
    agent: ureq::Agent,
    slots: Mutex<Vec<Slot>>,
    /// Worker threads still running, including ones left behind by pause
    /// or cancel; only used to share the bandwidth cap.
    active: AtomicUsize,
    max_concurrent: AtomicUsize,
    /// Bytes per second across all downloads; 0 = unlimited.
    bandwidth: AtomicU64,
    on_change: Listener,
    idle: Condvar,
}

#[derive(Clone)]
pub struct DownloadManager {
    inner: Arc<Inner>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// Keeps a download's file name safe for Windows.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '.') {
        "download".to_owned()
    } else {
        trimmed
    }
}

fn net(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Network, message)
}

/// A failed transfer attempt and whether retrying can help.
struct Attempt {
    error: AppError,
    retry: bool,
}

impl From<AppError> for Attempt {
    fn from(error: AppError) -> Self {
        let retry = matches!(error.code, ErrorCode::Network | ErrorCode::RateLimited);
        Self { error, retry }
    }
}

fn fatal(message: impl Into<String>) -> Attempt {
    Attempt {
        error: net(message),
        retry: false,
    }
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

impl DownloadManager {
    pub fn new(
        dir: &Path,
        max_concurrent: usize,
        on_change: impl Fn(&DownloadItem) + Send + Sync + 'static,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(format!(
                "KombiniModManager/{} (Deadlock mod manager; Rust)",
                env!("CARGO_PKG_VERSION")
            ))
            .http_status_as_error(false)
            .timeout_resolve(Some(Duration::from_secs(15)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .max_redirects(8)
            .build()
            .into();
        Self {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                agent,
                slots: Mutex::new(Vec::new()),
                active: AtomicUsize::new(0),
                max_concurrent: AtomicUsize::new(max_concurrent.clamp(1, 6)),
                bandwidth: AtomicU64::new(0),
                on_change: Arc::new(on_change),
                idle: Condvar::new(),
            }),
        }
    }

    pub fn set_max_concurrent(&self, n: usize) {
        self.inner
            .max_concurrent
            .store(n.clamp(1, 6), Ordering::SeqCst);
        self.pump();
    }

    /// Total bandwidth cap in KiB/s (`None` = unlimited).
    pub fn set_bandwidth_kbps(&self, kbps: Option<u32>) {
        self.inner.bandwidth.store(
            kbps.map(|k| u64::from(k) * 1024).unwrap_or(0),
            Ordering::SeqCst,
        );
    }

    pub fn items(&self) -> Vec<DownloadItem> {
        self.inner
            .slots
            .lock()
            .iter()
            .map(|s| s.item.clone())
            .collect()
    }

    pub fn item(&self, id: &str) -> Option<DownloadItem> {
        self.inner
            .slots
            .lock()
            .iter()
            .find(|s| s.item.spec.id == id)
            .map(|s| s.item.clone())
    }

    /// Re-adds persisted items after a restart. Interrupted downloads resume
    /// from their `.part`; finished ones stay as history.
    pub fn restore(&self, items: Vec<DownloadItem>) {
        {
            let mut slots = self.inner.slots.lock();
            for mut item in items {
                if slots.iter().any(|s| s.item.spec.id == item.spec.id) {
                    continue;
                }
                if item.status.is_active() {
                    item.status = Status::Queued;
                }
                item.bytes_per_sec = 0;
                slots.push(Slot {
                    item,
                    control: Arc::new(AtomicU8::new(RUN)),
                });
            }
        }
        self.pump();
    }

    /// Queues a download. An id that is already queued or running is ignored.
    pub fn enqueue(&self, spec: DownloadSpec) -> DownloadItem {
        let item = {
            let mut slots = self.inner.slots.lock();
            if let Some(existing) = slots.iter_mut().find(|s| s.item.spec.id == spec.id) {
                if !existing.item.status.is_finished() {
                    return existing.item.clone();
                }
                existing.item = DownloadItem {
                    spec,
                    status: Status::Queued,
                    received: 0,
                    total: None,
                    bytes_per_sec: 0,
                    error: None,
                    path: None,
                    created_at: now(),
                };
                existing.control.store(RUN, Ordering::SeqCst);
                existing.item.clone()
            } else {
                let item = DownloadItem {
                    spec,
                    status: Status::Queued,
                    received: 0,
                    total: None,
                    bytes_per_sec: 0,
                    error: None,
                    path: None,
                    created_at: now(),
                };
                slots.push(Slot {
                    item: item.clone(),
                    control: Arc::new(AtomicU8::new(RUN)),
                });
                item
            }
        };
        (self.inner.on_change)(&item);
        self.pump();
        item
    }

    /// Pauses a queued or running download; the partial file is kept.
    pub fn pause(&self, id: &str) {
        let item = {
            let mut slots = self.inner.slots.lock();
            let Some(slot) = slots.iter_mut().find(|s| s.item.spec.id == id) else {
                return;
            };
            if !matches!(slot.item.status, Status::Queued | Status::Downloading) {
                return;
            }
            slot.control.store(PAUSE, Ordering::SeqCst);
            slot.item.status = Status::Paused;
            slot.item.bytes_per_sec = 0;
            slot.item.clone()
        };
        (self.inner.on_change)(&item);
        self.pump();
    }

    /// Resumes a paused download or retries a failed one. A failed item
    /// whose file was already downloaded (a later step such as installing
    /// failed) goes straight back to `Completed` without downloading again.
    pub fn resume(&self, id: &str) {
        let item = {
            let mut slots = self.inner.slots.lock();
            let Some(slot) = slots.iter_mut().find(|s| s.item.spec.id == id) else {
                return;
            };
            match slot.item.status {
                Status::Paused => slot.item.status = Status::Queued,
                Status::Failed => {
                    if slot.item.path.as_deref().is_some_and(Path::is_file) {
                        slot.item.status = Status::Completed;
                    } else {
                        slot.item.status = Status::Queued;
                        slot.item.path = None;
                    }
                }
                _ => return,
            }
            slot.item.error = None;
            slot.item.bytes_per_sec = 0;
            slot.item.clone()
        };
        (self.inner.on_change)(&item);
        self.pump();
    }

    /// Cancels a download and deletes its partial data, at once even when
    /// the transfer is stuck.
    pub fn cancel(&self, id: &str) {
        let item = {
            let mut slots = self.inner.slots.lock();
            let Some(slot) = slots.iter_mut().find(|s| s.item.spec.id == id) else {
                return;
            };
            if slot.item.status.is_finished() {
                return;
            }
            slot.control.store(CANCEL, Ordering::SeqCst);
            slot.item.status = Status::Cancelled;
            slot.item.bytes_per_sec = 0;
            slot.item.clone()
        };
        self.discard_part(id);
        (self.inner.on_change)(&item);
        self.pump();
    }

    /// Marks a completed download failed because a later step (installing)
    /// failed. The file is kept, so `resume` retries that step without
    /// downloading again.
    pub fn fail(&self, id: &str, error: AppError) {
        let item = {
            let mut slots = self.inner.slots.lock();
            let Some(slot) = slots
                .iter_mut()
                .find(|s| s.item.spec.id == id && s.item.status == Status::Completed)
            else {
                return;
            };
            slot.item.status = Status::Failed;
            slot.item.error = Some(error);
            slot.item.clone()
        };
        (self.inner.on_change)(&item);
    }

    /// Deletes the partial file. It is renamed first: a worker stuck in a
    /// read may still hold it open, and a new download of the same id must
    /// be able to create the path again.
    fn discard_part(&self, id: &str) {
        let part = self.part_path(id);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let gone = part.with_extension(format!("part.{stamp}.old"));
        match std::fs::rename(&part, &gone) {
            Ok(()) => {
                let _ = std::fs::remove_file(&gone);
            }
            Err(_) => {
                let _ = std::fs::remove_file(&part);
            }
        }
    }

    /// Runs `f` on the slot of `id` only while `control` is still that
    /// slot's token and the item is running; reports from a worker that
    /// was paused, cancelled or replaced are dropped.
    fn with_current(
        &self,
        id: &str,
        control: &Arc<AtomicU8>,
        f: impl FnOnce(&mut DownloadItem),
    ) -> bool {
        let item = {
            let mut slots = self.inner.slots.lock();
            let Some(slot) = slots.iter_mut().find(|s| {
                s.item.spec.id == id
                    && Arc::ptr_eq(&s.control, control)
                    && s.item.status.is_active()
            }) else {
                return false;
            };
            f(&mut slot.item);
            slot.item.clone()
        };
        (self.inner.on_change)(&item);
        true
    }

    /// Removes a finished item from the list (keeps the downloaded file).
    /// Fires `on_change` with the removed item so the list is saved and redrawn.
    pub fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut slots = self.inner.slots.lock();
            let pos = slots
                .iter()
                .position(|s| s.item.spec.id == id && s.item.status.is_finished());
            pos.map(|p| slots.remove(p).item)
        };
        match removed {
            Some(item) => {
                (self.inner.on_change)(&item);
                true
            }
            None => false,
        }
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{}.part", safe_file_name(id)))
    }

    fn final_path(&self, spec: &DownloadSpec) -> PathBuf {
        self.inner
            .dir
            .join(safe_file_name(&spec.id))
            .join(safe_file_name(&spec.file_name))
    }

    /// Blocks until nothing is queued or running (tests, shutdown).
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut slots = self.inner.slots.lock();
        loop {
            let busy = slots
                .iter()
                .any(|s| matches!(s.item.status, Status::Queued) || s.item.status.is_active());
            if !busy {
                return true;
            }
            if self.inner.idle.wait_until(&mut slots, deadline).timed_out() {
                return false;
            }
        }
    }

    /// Starts workers for queued items up to the concurrency limit. Only
    /// items still marked running count, so a worker stuck after pause or
    /// cancel does not hold up the queue.
    fn pump(&self) {
        loop {
            let (item, control) = {
                let mut slots = self.inner.slots.lock();
                let running = slots.iter().filter(|s| s.item.status.is_active()).count();
                if running >= self.inner.max_concurrent.load(Ordering::SeqCst) {
                    return;
                }
                let Some(slot) = slots.iter_mut().find(|s| s.item.status == Status::Queued) else {
                    self.inner.idle.notify_all();
                    return;
                };
                slot.item.status = Status::Downloading;
                slot.item.error = None;
                slot.control = Arc::new(AtomicU8::new(RUN));
                (slot.item.clone(), slot.control.clone())
            };
            (self.inner.on_change)(&item);
            self.inner.active.fetch_add(1, Ordering::SeqCst);
            let me = self.clone();
            let worker_control = control.clone();
            let spec = item.spec.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("download-{}", item.spec.id))
                .spawn(move || {
                    let result = me.run(&spec, &worker_control);
                    me.finish(&spec.id, &worker_control, result);
                    me.inner.active.fetch_sub(1, Ordering::SeqCst);
                    me.pump();
                    me.inner.idle.notify_all();
                });
            if let Err(e) = spawned {
                self.inner.active.fetch_sub(1, Ordering::SeqCst);
                self.finish(
                    &item.spec.id,
                    &control,
                    Err(AppError::internal(format!("thread: {e}"))),
                );
            }
        }
    }

    fn finish(&self, id: &str, control: &Arc<AtomicU8>, result: Result<PathBuf, AppError>) {
        let done = self.with_current(id, control, |item| {
            item.bytes_per_sec = 0;
            match result {
                Ok(path) => {
                    item.status = Status::Completed;
                    item.path = Some(path);
                    item.received = item.total.unwrap_or(item.received);
                }
                Err(e) if e.code == ErrorCode::Cancelled => {
                    item.status = match control.load(Ordering::SeqCst) {
                        PAUSE => Status::Paused,
                        _ => Status::Cancelled,
                    };
                }
                Err(e) => {
                    item.status = Status::Failed;
                    item.error = Some(e);
                }
            }
        });
        if done && self.item(id).is_some_and(|i| i.status == Status::Cancelled) {
            self.discard_part(id);
        }
    }

    fn update_progress(
        &self,
        id: &str,
        control: &Arc<AtomicU8>,
        received: u64,
        total: Option<u64>,
        bps: u64,
    ) {
        self.with_current(id, control, |item| {
            item.received = received;
            item.total = total;
            item.bytes_per_sec = bps;
        });
    }

    /// Sleeps `secs`, waking early (with `Cancelled`) on pause or cancel.
    fn wait(control: &AtomicU8, secs: u64) -> Result<(), AppError> {
        for _ in 0..secs * 10 {
            if control.load(Ordering::SeqCst) != RUN {
                return Err(AppError::cancelled());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn run(&self, spec: &DownloadSpec, control: &Arc<AtomicU8>) -> Result<PathBuf, AppError> {
        std::fs::create_dir_all(&self.inner.dir)
            .map_err(|e| AppError::from_io("Creating the downloads folder", &self.inner.dir, &e))?;
        let part = self.part_path(&spec.id);
        // A partial file that fails verification (stale or corrupt) is
        // deleted and downloaded once more from the start.
        let mut fresh_start = false;
        loop {
            self.fetch(spec, &part, control)?;
            if !self.with_current(&spec.id, control, |item| {
                item.status = Status::Verifying;
                item.bytes_per_sec = 0;
            }) {
                return Err(AppError::cancelled());
            }
            match verify(&part, spec) {
                Ok(()) => break,
                Err(e) => {
                    let _ = std::fs::remove_file(&part);
                    if fresh_start {
                        return Err(e);
                    }
                    tracing::warn!(id = %spec.id, error = %e, "download failed verification; starting over");
                    fresh_start = true;
                    if !self.with_current(&spec.id, control, |item| {
                        item.status = Status::Downloading;
                        item.received = 0;
                    }) {
                        return Err(AppError::cancelled());
                    }
                }
            }
        }
        let dest = self.final_path(spec);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::from_io("Saving the download", parent, &e))?;
        }
        if dest.exists() {
            let _ = std::fs::remove_file(&dest);
        }
        std::fs::rename(&part, &dest)
            .map_err(|e| AppError::from_io("Saving the download", &dest, &e))?;
        Ok(dest)
    }

    /// Transfers into `part`, retrying network failures with backoff.
    fn fetch(
        &self,
        spec: &DownloadSpec,
        part: &Path,
        control: &Arc<AtomicU8>,
    ) -> Result<(), AppError> {
        let mut attempt = 0;
        loop {
            let before = file_len(part);
            let failure = match self.transfer(spec, part, control) {
                Ok(()) => return Ok(()),
                Err(f) => f,
            };
            if !failure.retry || control.load(Ordering::SeqCst) != RUN {
                return Err(failure.error);
            }
            if file_len(part) > before {
                // Progress was made: a flaky connection, not a dead one.
                attempt = 0;
            }
            let Some(&backoff) = BACKOFF_SECS.get(attempt) else {
                return Err(failure.error);
            };
            attempt += 1;
            let secs = if failure.error.code == ErrorCode::RateLimited {
                RATE_LIMIT_WAIT_SECS
            } else {
                backoff
            };
            tracing::warn!(id = %spec.id, attempt, secs, error = %failure.error, "download interrupted; retrying");
            Self::wait(control, secs)?;
        }
    }

    /// One HTTP attempt, appending to `part`.
    fn transfer(
        &self,
        spec: &DownloadSpec,
        part: &Path,
        control: &Arc<AtomicU8>,
    ) -> Result<(), Attempt> {
        let have = file_len(part);
        if spec.expected_size.is_some_and(|s| s > 0 && have == s) {
            return Ok(());
        }
        let mut req = self.inner.agent.get(&spec.url).header("Accept", "*/*");
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let resp = req
            .call()
            .map_err(|e| net(format!("Download failed: {e}")))?;
        if control.load(Ordering::SeqCst) != RUN {
            return Err(AppError::cancelled().into());
        }
        let status = resp.status().as_u16();
        let (append, start) = match status {
            206 => (true, have),
            200 => (false, 0),
            416 if have > 0 => {
                // Range not satisfiable: the part may already be complete.
                return Ok(());
            }
            404 | 410 => {
                return Err(fatal(
                    "The file is no longer available on the server (it may have been removed or archived)",
                ));
            }
            429 => {
                return Err(AppError::new(
                    ErrorCode::RateLimited,
                    "The server is rate limiting downloads; try again in a minute",
                )
                .into());
            }
            s if s >= 500 || s == 408 => return Err(net(format!("Server error (HTTP {s})")).into()),
            s => return Err(fatal(format!("Unexpected response (HTTP {s})"))),
        };
        let content_len = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let total = content_len.map(|l| l + start).or(spec.expected_size);
        if total.is_some_and(|t| t > MAX_DOWNLOAD_BYTES) {
            return Err(
                AppError::new(ErrorCode::ArchiveTooLarge, "The download is too large").into(),
            );
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .map_err(|e| AppError::from_io("Writing the download", part, &e))?;
        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; CHUNK];
        let mut received = start;
        let started = Instant::now();
        let mut window_start = Instant::now();
        let mut window_bytes = 0u64;
        let mut last_event = Instant::now();
        let mut bps = 0u64;
        self.update_progress(&spec.id, control, received, total, 0);
        loop {
            if control.load(Ordering::SeqCst) != RUN {
                let _ = file.flush();
                return Err(AppError::cancelled().into());
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| net(format!("Connection lost: {e}")))?;
            if n == 0 {
                break;
            }
            // A read can block for a long time; don't write after a pause
            // or cancel, when another run may own the file.
            if control.load(Ordering::SeqCst) != RUN {
                return Err(AppError::cancelled().into());
            }
            file.write_all(&buf[..n])
                .map_err(|e| AppError::from_io("Writing the download", part, &e))?;
            received += n as u64;
            window_bytes += n as u64;
            if received > MAX_DOWNLOAD_BYTES {
                return Err(
                    AppError::new(ErrorCode::ArchiveTooLarge, "The download is too large").into(),
                );
            }
            // Bandwidth cap, shared evenly between active downloads.
            let cap = self.inner.bandwidth.load(Ordering::SeqCst);
            if cap > 0 {
                let share = cap / self.inner.active.load(Ordering::SeqCst).max(1) as u64;
                let expected = Duration::from_secs_f64(window_bytes as f64 / share.max(1) as f64);
                let elapsed = window_start.elapsed();
                if expected > elapsed {
                    std::thread::sleep(expected - elapsed);
                }
            }
            if window_start.elapsed() >= Duration::from_secs(1) {
                bps = (window_bytes as f64 / window_start.elapsed().as_secs_f64()) as u64;
                window_start = Instant::now();
                window_bytes = 0;
            } else if bps == 0 && started.elapsed() > Duration::from_millis(250) {
                bps = (received.saturating_sub(start) as f64 / started.elapsed().as_secs_f64())
                    as u64;
            }
            if last_event.elapsed() >= EVENT_INTERVAL {
                self.update_progress(&spec.id, control, received, total, bps);
                last_event = Instant::now();
            }
        }
        file.sync_all()
            .map_err(|e| AppError::from_io("Writing the download", part, &e))?;
        self.update_progress(&spec.id, control, received, total, 0);
        if let Some(t) = total
            && received < t
        {
            return Err(net("The connection closed before the download finished").into());
        }
        Ok(())
    }
}

/// Streams the file through MD5 and checks the expected size.
pub fn md5_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn verify(part: &Path, spec: &DownloadSpec) -> Result<(), AppError> {
    let size = std::fs::metadata(part)
        .map_err(|e| AppError::from_io("Checking the download", part, &e))?
        .len();
    if let Some(expected) = spec.expected_size.filter(|s| *s > 0)
        && size != expected
    {
        return Err(AppError::new(
            ErrorCode::HashMismatch,
            format!("The download is incomplete ({size} of {expected} bytes); it will restart"),
        ));
    }
    if let Some(expected) = spec.expected_md5.as_deref().filter(|m| !m.is_empty()) {
        let actual =
            md5_file(part).map_err(|e| AppError::from_io("Checking the download", part, &e))?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(AppError::new(
                ErrorCode::HashMismatch,
                "The downloaded file is corrupt (checksum mismatch); try again",
            )
            .with_detail(format!("expected {expected}, got {actual}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
