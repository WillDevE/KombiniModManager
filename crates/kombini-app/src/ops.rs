//! Operations shown in the activity center. Engine work reports progress
//! through [`OpProgress`] (lock-free atomics + a stage label); the UI samples
//! it on its 10 Hz tick, so progress updates are coalesced for free.

use kombini_engine::Progress;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Operations shorter than this never show progress UI.
pub const SHOW_AFTER: Duration = Duration::from_millis(150);

#[derive(Default)]
pub struct OpShared {
    stage: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

#[derive(Clone, Default)]
pub struct OpProgress(pub Arc<OpShared>);

impl OpProgress {
    pub fn stage_label(&self) -> String {
        self.0.stage.lock().clone()
    }

    /// `None` while indeterminate.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.0.total.load(Ordering::Relaxed);
        (total > 0).then(|| {
            let done = self.0.done.load(Ordering::Relaxed).min(total);
            done as f32 / total as f32
        })
    }

    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::Relaxed)
    }
}

impl Progress for OpProgress {
    fn stage(&self, label: &str) {
        label.clone_into(&mut self.0.stage.lock());
        self.0.done.store(0, Ordering::Relaxed);
        self.0.total.store(0, Ordering::Relaxed);
    }
    fn progress(&self, done: u64, total: u64) {
        self.0.done.store(done, Ordering::Relaxed);
        self.0.total.store(total, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

pub struct Op {
    pub key: String,
    pub label: String,
    pub started: Instant,
    pub progress: OpProgress,
    pub cancellable: bool,
}

impl Op {
    pub fn visible(&self) -> bool {
        self.started.elapsed() >= SHOW_AFTER
    }
}

/// Finished operation kept for the activity center history.
#[derive(Clone)]
pub struct OpRecord {
    pub label: String,
    pub ok: bool,
    pub message: String,
    pub took: Duration,
    pub at: Instant,
}

pub const HISTORY: usize = 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_fraction_and_cancel() {
        let p = OpProgress::default();
        assert_eq!(p.fraction(), None);
        p.stage("Copying");
        p.progress(5, 10);
        assert_eq!(p.fraction(), Some(0.5));
        assert_eq!(p.stage_label(), "Copying");
        p.progress(20, 10);
        assert_eq!(p.fraction(), Some(1.0));
        assert!(!p.cancelled());
        p.cancel();
        assert!(p.cancelled());
    }
}
