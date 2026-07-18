//! Per-render resolve-failure tracker. Jinja functions record every
//! `failures[]` entry from aggregate responses; the MCP server reads
//! `snapshot()` to surface them out-of-band.

use std::sync::{Arc, Mutex};

/// Collects per-ingredient resolve failures raised during a render, so
/// callers (the MCP server) can surface them out-of-band. Failures are stored
/// as raw JSON — the service's `code`/`message`/`suggestions` shape is the
/// contract and must pass through unnarrowed.
#[derive(Default, Clone)]
pub struct FailureTracker {
    inner: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl FailureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, failure: serde_json::Value) {
        self.inner.lock().unwrap().push(failure);
    }

    pub fn snapshot(&self) -> Vec<serde_json::Value> {
        self.inner.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        self.inner.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_snapshots() {
        let t = FailureTracker::new();
        t.record(serde_json::json!({"index": 1, "error": {"code": "density_unavailable"}}));
        let snap = t.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0]["error"]["code"], "density_unavailable");
        t.reset();
        assert!(t.snapshot().is_empty());
    }

    #[test]
    fn clone_shares_state() {
        let t = FailureTracker::new();
        let t2 = t.clone();
        t.record(serde_json::json!({"index": 0}));
        assert_eq!(t2.snapshot().len(), 1);
    }
}
