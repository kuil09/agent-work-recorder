//! Capture telemetry is distinct from encoded copies of the last source frame.
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub const HEALTH_TIMEOUT_MS: u64 = 3000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureInterval {
    pub reason: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureHealth {
    pub state: String,
    pub first_frame: String,
    pub target_available: Option<bool>,
    pub frames_received: u64,
    pub frames_written: u64,
    pub last_frame_ms: Option<u64>,
    pub last_frame_age_ms: Option<u64>,
    pub last_sample_age_ms: Option<u64>,
    pub capture_error: Option<String>,
    pub visual_warning: Option<String>,
    pub intervals: Vec<CaptureInterval>,
}

pub struct HealthTracker {
    value: CaptureHealth,
    updated: Option<Instant>,
}
impl Default for HealthTracker {
    fn default() -> Self {
        Self {
            value: CaptureHealth {
                state: "unverified".into(),
                first_frame: "pending".into(),
                ..Default::default()
            },
            updated: None,
        }
    }
}
impl HealthTracker {
    pub fn update(&mut self, mut value: CaptureHealth, now: Instant, elapsed_ms: u64) {
        let _ = self.snapshot(now, elapsed_ms);
        value.intervals = std::mem::take(&mut self.value.intervals);
        self.value = value;
        self.updated = Some(now);
        self.reconcile(elapsed_ms, false);
    }
    pub fn error(&mut self, message: String, now: Instant, elapsed_ms: u64) {
        self.value.capture_error = Some(message);
        // A process error must not establish target disappearance.
        self.reconcile(
            elapsed_ms,
            self.updated
                .map(|t| now.duration_since(t).as_millis() as u64 > HEALTH_TIMEOUT_MS)
                .unwrap_or(false),
        );
    }
    pub fn target_lost(&mut self, elapsed_ms: u64) {
        self.value.target_available = Some(false);
        self.reconcile(elapsed_ms, false);
    }
    fn interval(&mut self, reason: &str, active: bool, elapsed_ms: u64) {
        let open = self
            .value
            .intervals
            .iter_mut()
            .rev()
            .find(|i| i.reason == reason && i.end_ms.is_none());
        if active {
            if open.is_none() {
                self.value.intervals.push(CaptureInterval {
                    reason: reason.into(),
                    start_ms: elapsed_ms,
                    end_ms: None,
                });
            }
        } else if let Some(open) = open {
            open.end_ms = Some(elapsed_ms);
        }
    }
    fn reconcile(&mut self, elapsed_ms: u64, stale: bool) {
        let lost = self.value.target_available == Some(false);
        let missing = !stale
            && self.value.first_frame == "validated"
            && self.updated.is_some()
            && self
                .value
                .last_sample_age_ms
                .map(|age| age > HEALTH_TIMEOUT_MS)
                .unwrap_or(true);
        let error = self.value.capture_error.is_some();
        self.interval("target_lost", lost, elapsed_ms);
        self.interval("missing_frames", missing, elapsed_ms);
        self.interval("capture_error", error, elapsed_ms);
        self.interval("telemetry_unavailable", stale, elapsed_ms);
        self.value.state = if error {
            "capture_error"
        } else if lost {
            "target_lost"
        } else if stale {
            "telemetry_unavailable"
        } else if missing {
            "missing_frames"
        } else if self.value.first_frame != "validated" || self.value.target_available.is_none() {
            "unverified"
        } else if self.value.visual_warning.is_some() {
            "visual_warning"
        } else {
            "receiving"
        }
        .into();
    }
    pub fn snapshot(&mut self, now: Instant, elapsed_ms: u64) -> CaptureHealth {
        let age = self
            .updated
            .map(|t| now.saturating_duration_since(t).as_millis() as u64);
        let mut value = self.value.clone();
        if let Some(age) = age {
            self.value.last_sample_age_ms = value.last_sample_age_ms.map(|a| a.saturating_add(age));
        }
        let stale = age.map(|age| age > HEALTH_TIMEOUT_MS).unwrap_or(false);
        self.reconcile(elapsed_ms, stale);
        self.value.last_sample_age_ms = value.last_sample_age_ms;
        value = self.value.clone();
        if let Some(age) = age {
            value.last_frame_age_ms = value.last_frame_age_ms.map(|a| a.saturating_add(age));
            value.last_sample_age_ms = value.last_sample_age_ms.map(|a| a.saturating_add(age));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn valid() -> CaptureHealth {
        CaptureHealth {
            first_frame: "validated".into(),
            target_available: Some(true),
            frames_received: 1,
            frames_written: 30,
            last_frame_ms: Some(0),
            last_frame_age_ms: Some(0),
            last_sample_age_ms: Some(0),
            ..Default::default()
        }
    }
    #[test]
    fn target_loss_and_recovery_have_spans() {
        let now = Instant::now();
        let mut t = HealthTracker::default();
        t.update(valid(), now, 0);
        let mut lost = valid();
        lost.target_available = Some(false);
        t.update(lost, now, 1000);
        assert_eq!(t.snapshot(now, 1000).state, "target_lost");
        t.update(valid(), now, 2500);
        let h = t.snapshot(now, 2500);
        assert_eq!(h.intervals[0].start_ms, 1000);
        assert_eq!(h.intervals[0].end_ms, Some(2500));
    }
    #[test]
    fn cached_encoded_frames_do_not_hide_missing_samples() {
        let now = Instant::now();
        let mut t = HealthTracker::default();
        t.update(valid(), now, 0);
        let h = t.snapshot(now + Duration::from_secs(4), 4000);
        assert_eq!(h.state, "telemetry_unavailable");
        assert!(!h.intervals.iter().any(|i| i.reason == "missing_frames"));
        let mut stalled = valid();
        stalled.frames_written = 120;
        stalled.last_sample_age_ms = Some(4000);
        t.update(stalled, now + Duration::from_secs(4), 4000);
        assert_eq!(
            t.snapshot(now + Duration::from_secs(4), 4000).state,
            "missing_frames"
        );
        let h = t.snapshot(now + Duration::from_secs(5), 5000);
        assert_eq!(h.last_sample_age_ms, Some(5000));
    }
    #[test]
    fn dark_static_warning_is_not_target_loss() {
        let now = Instant::now();
        let mut t = HealthTracker::default();
        let mut h = valid();
        h.visual_warning = Some("dark source pixels".into());
        t.update(h, now, 0);
        let h = t.snapshot(now, 0);
        assert_eq!(h.state, "visual_warning");
        assert_eq!(h.target_available, Some(true));
        assert!(h.intervals.is_empty());
        t.error("stream failed".into(), now, 1000);
        assert_eq!(t.snapshot(now, 1000).target_available, Some(true));
    }
    #[test]
    fn no_telemetry_is_never_healthy() {
        let mut t = HealthTracker::default();
        assert_eq!(t.snapshot(Instant::now(), 0).first_frame, "pending");
        assert_ne!(t.snapshot(Instant::now(), 0).state, "receiving");
    }
}
