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
            ObserveStatus::Pass => Some("Agent verdict: PASS"),
            ObserveStatus::Fail => Some("Agent verdict: FAIL"),
            ObserveStatus::Uncertain => Some("Agent verdict: UNCERTAIN"),
            ObserveStatus::Info => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ObserveStatus::Pass => "pass",
            ObserveStatus::Fail => "fail",
            ObserveStatus::Uncertain => "uncertain",
            ObserveStatus::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Note,
    Expect,
    Observe,
    Checkpoint,
    Git,
}

impl EventKind {
    pub fn as_card_title(self) -> &'static str {
        match self {
            EventKind::Note => "NOTE",
            EventKind::Expect => "EXPECT",
            EventKind::Observe => "OBSERVE",
            EventKind::Checkpoint => "CHECKPOINT",
            EventKind::Git => "CONTEXT",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub ts_ms: u64,
    pub step: u32,
    pub step_id: String,
    pub kind: EventKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ObserveStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum IpcRequest {
    Note {
        text: String,
    },
    Expect {
        text: String,
    },
    Observe {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ObserveStatus>,
    },
    Checkpoint {
        text: String,
    },
    Stop,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub verdict: Option<String>,
}

impl IpcResponse {
    pub fn err(msg: impl Into<String>) -> Self {
        IpcResponse {
            ok: false,
            error: Some(msg.into()),
            run_id: None,
            step: None,
            step_id: None,
            kind: None,
            output: None,
            duration: None,
            steps: None,
            checkpoints: None,
            verdict: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureHud {
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    }

    #[test]
    fn ipc_roundtrip() {
        let req = IpcRequest::Observe {
            text: "ok".into(),
            status: Some(ObserveStatus::Pass),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: IpcRequest = serde_json::from_str(&json).unwrap();
        match back {
            IpcRequest::Observe { text, status } => {
                assert_eq!(text, "ok");
                assert_eq!(status, Some(ObserveStatus::Pass));
            }
            _ => panic!("wrong variant"),
        }
    }
}
