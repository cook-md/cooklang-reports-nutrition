//! Per-render check tracker. Macros call `record_check(label, ok)` and the
//! demo reads `failed_count()` to set its exit code.

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CheckRecord {
    pub label: String,
    pub ok: bool,
}

#[derive(Default, Clone)]
pub struct CheckTracker {
    inner: Arc<Mutex<Vec<CheckRecord>>>,
}

impl CheckTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, label: impl Into<String>, ok: bool) {
        self.inner.lock().unwrap().push(CheckRecord {
            label: label.into(),
            ok,
        });
    }

    pub fn snapshot(&self) -> Vec<CheckRecord> {
        self.inner.lock().unwrap().clone()
    }

    pub fn failed_count(&self) -> usize {
        self.inner.lock().unwrap().iter().filter(|c| !c.ok).count()
    }

    pub fn reset(&self) {
        self.inner.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_preserve_order_and_status() {
        let t = CheckTracker::new();
        t.record("a", true);
        t.record("b", false);
        t.record("c", true);
        let s = t.snapshot();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].label, "a");
        assert!(s[0].ok);
        assert_eq!(s[1].label, "b");
        assert!(!s[1].ok);
        assert_eq!(s[2].label, "c");
    }

    #[test]
    fn failed_count_counts_only_failures() {
        let t = CheckTracker::new();
        t.record("a", true);
        t.record("b", false);
        t.record("c", false);
        t.record("d", true);
        assert_eq!(t.failed_count(), 2);
    }

    #[test]
    fn snapshot_is_a_clone_not_aliased() {
        let t = CheckTracker::new();
        t.record("a", true);
        let mut s = t.snapshot();
        s.clear();
        assert_eq!(t.snapshot().len(), 1);
    }

    #[test]
    fn reset_clears_records() {
        let t = CheckTracker::new();
        t.record("a", true);
        t.record("b", false);
        t.reset();
        assert_eq!(t.snapshot().len(), 0);
        assert_eq!(t.failed_count(), 0);
    }

    #[test]
    fn clone_shares_state() {
        let t = CheckTracker::new();
        let t2 = t.clone();
        t.record("a", true);
        assert_eq!(t2.snapshot().len(), 1);
    }
}
