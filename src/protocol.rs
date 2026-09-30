use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObserveStatus {
    Pass,
    Fail,
    Uncertain,
    Info,
}
impl ObserveStatus {
    pub fn as_verdict_label(self) -> Option<&'static str> {
        match self {
            Self::Pass => Some("Agent verdict: PASS"),
            Self::Fail => Some("Agent verdict: FAIL"),
            Self::Uncertain => Some("Agent verdict: UNCERTAIN"),
            Self::Info => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Uncertain => "uncertain",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Note,
    Expect,
    Observe,
    Checkpoint,
    Git,
    TestStart,
    TestResult,
}
impl EventKind {
    pub fn as_card_title(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Expect => "EXPECT",
            Self::Observe => "OBSERVE",
            Self::Checkpoint => "CHECKPOINT",
            Self::Git => "CONTEXT",
            Self::TestStart | Self::TestResult => "TEST",
        }
    }
}

/// Mechanical command results, never an agent verdict. Output summaries are bounded tails.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestResult {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub stdout_summary: String,
    pub stderr_summary: String,
    pub error: Option<String>,
}
impl TestResult {
    pub fn cli_exit_code(&self) -> i32 {
        if self.timed_out {
            124
        } else if self.error.is_some() {
            127
        } else {
            self.exit_code
                .unwrap_or_else(|| 128 + self.signal.unwrap_or(1))
        }
    }
    pub fn card_body(&self, command: &str) -> String {
        let exit = self.exit_code.map(|n| n.to_string()).unwrap_or_else(|| {
            self.signal
                .map(|n| format!("signal {n}"))
                .unwrap_or_else(|| "unavailable".into())
        });
        let mut s = format!(
            "Command: {}\nExit: {exit} | Duration: {:.3}s{}",
            crate::runner::display_text(command, 200),
            self.duration_ms as f64 / 1000.0,
            if self.timed_out { " | TIMEOUT" } else { "" }
        );
        if let Some(e) = &self.error {
            s.push_str(&format!("\nError: {}", crate::runner::display_text(e, 200)));
        }
        for (label, value) in [
            ("stdout", &self.stdout_summary),
            ("stderr", &self.stderr_summary),
        ] {
            if !value.is_empty() {
                s.push_str(&format!(
                    "\n{label}: {}",
                    crate::runner::display_text(value, 180)
                ));
            }
        }
        s
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub ts_ms: u64,
    /// Monotonic milliseconds on the capture timeline, not wall-clock subtraction.
    pub media_ms: u64,
    pub step: u32,
    pub step_id: String,
    pub kind: EventKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ObserveStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_result: Option<TestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    Note {
        text: String,
    },
    Expect {
        text: String,
    },
    Observe {
        text: String,
        status: Option<ObserveStatus>,
    },
    Checkpoint {
        text: String,
    },
    TestBegin {
        command: Vec<String>,
        cwd: String,
        owner_pid: u32,
        /// Client-side timeout; the daemon derives a deadline from it. 0 = unknown (old client).
        #[serde(default)]
        timeout_secs: u64,
    },
    TestEnd {
        run_id: String,
        test_step: u32,
        result: TestResult,
    },
    Stop {
        /// Close a still-running test as "result unknown" before stopping.
        #[serde(default)]
        abandon_test: bool,
    },
    Status,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_test: Option<String>,
}
impl IpcResponse {
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            error: Some(msg.into()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureHud {
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    // Null MUST be transmitted: omitting this field leaves a stale PASS in the helper.
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pass_is_claim_not_truth() {
        assert_eq!(
            ObserveStatus::Pass.as_verdict_label(),
            Some("Agent verdict: PASS")
        );
        assert_eq!(ObserveStatus::Info.as_verdict_label(), None);
        assert!(!TestResult {
            exit_code: Some(0),
            ..Default::default()
        }
        .card_body("true")
        .contains("PASS"));
    }
    #[test]
    fn ipc_roundtrip() {
        let req = IpcRequest::Observe {
            text: "ok".into(),
            status: Some(ObserveStatus::Pass),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(matches!(
            serde_json::from_str::<IpcRequest>(&json).unwrap(),
            IpcRequest::Observe {
                status: Some(ObserveStatus::Pass),
                ..
            }
        ));
    }
    #[test]
    fn clearing_verdict_is_explicit() {
        let json = serde_json::to_value(CaptureHud {
            cmd: "hud".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(json.as_object().unwrap().contains_key("verdict"));
        assert!(json["verdict"].is_null());
    }
}
