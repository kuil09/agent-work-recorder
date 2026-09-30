//! Agent-facing help is a tested interface, not an assertion of recording quality.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const COMMANDS: &[&str] = &[
    "start",
    "note",
    "expect",
    "observe",
    "checkpoint",
    "test",
    "stop",
    "status",
];
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("rec-help-{}-{serial}", std::process::id()));
        fs::create_dir(&root).unwrap();
        for dir in ["home", "tmp", "work"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        Self(root)
    }
    fn invoke(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rec"))
            .args(args)
            .env_clear()
            .env("PATH", "")
            .env("HOME", self.0.join("home"))
            .env("TMPDIR", self.0.join("tmp"))
            .env("REC_CAPTURE", self.0.join("must-not-run"))
            .current_dir(self.0.join("work"))
            .output()
            .unwrap()
    }
    fn help(&self, args: &[&str]) -> String {
        let result = self.invoke(args);
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.stderr.is_empty(),
            "help must not emit diagnostics: {args:?}"
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(
            !text.contains('\u{1b}'),
            "piped help must not contain ANSI escapes"
        );
        text
    }
    fn assert_untouched(&self) {
        for dir in ["home", "tmp", "work"] {
            assert_eq!(
                fs::read_dir(self.0.join(dir)).unwrap().count(),
                0,
                "help mutated {dir}"
            );
        }
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn contains_all(text: &str, needles: &[&str]) {
    let text = normalized(text);
    for needle in needles {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
}

#[test]
fn every_help_route_is_read_only_and_requires_no_capture_dependencies() {
    let s = Sandbox::new();
    for args in [vec!["-h"], vec!["--help"], vec!["help"], vec!["--version"]] {
        s.help(&args);
    }
    for command in COMMANDS {
        let short = s.help(&[command, "-h"]);
        let long = s.help(&[command, "--help"]);
        let via_help = s.help(&["help", command]);
        assert!(
            long.len() > short.len(),
            "{command} needs progressive detail"
        );
        assert_eq!(normalized(&long), normalized(&via_help));
        contains_all(&long, &["Usage:", "EXAMPLE"]);
    }
    s.assert_untouched();
}

#[test]
fn root_help_explains_discovery_ownership_claims_and_output() {
    let s = Sandbox::new();
    let help = s.help(&["--help"]);
    contains_all(
        &help,
        &[
            "AGENT WORKFLOW",
            "claims, not truth",
            "command exit 0",
            "HOME",
            "TMPDIR",
            "OUTPUT AND EXIT STATUS",
            "not a stable JSON API",
            "No --json",
            "SAFETY AND LIMITS",
            "SKILL.md",
            "rec <COMMAND> --help",
        ],
    );
    assert!(!help.lines().any(|l| l.trim_start().starts_with("daemon ")));
    s.assert_untouched();
}

#[test]
fn start_help_documents_every_option_and_capture_privacy_boundary() {
    let s = Sandbox::new();
    let help = s.help(&["start", "--help"]);
    contains_all(
        &help,
        &[
            "--title",
            "--output",
            "--screen",
            "--window",
            "--window-id",
            "--app",
            "--system-audio",
            "main display",
            "OTHER windows",
            "Default OFF",
            "never overwritten",
            "rec-capture list-windows",
            "rec-capture list-apps",
            "ON SUCCESS",
            "ON FAILURE",
        ],
    );
}

#[test]
fn annotations_distinguish_actions_expectations_claims_and_checkpoints() {
    let s = Sandbox::new();
    contains_all(
        &s.help(&["note", "--help"]),
        &[
            "one Step",
            "previous agent verdict",
            "4 seconds",
            "does not perform",
        ],
    );
    contains_all(
        &s.help(&["expect", "--help"]),
        &["Preserves", "before", "no chapter", "honest observe"],
    );
    contains_all(
        &s.help(&["observe", "--help"]),
        &[
            "pass",
            "fail",
            "uncertain",
            "info",
            "clears",
            "does not inspect the UI",
        ],
    );
    contains_all(
        &s.help(&["checkpoint", "--help"]),
        &["chapter candidate", "separate screenshot", "freeze", "stop"],
    );
}

#[test]
fn test_help_warns_about_argv_status_and_finalization() {
    let s = Sandbox::new();
    let help = s.help(&["test", "--help"]);
    contains_all(
        &help,
        &[
            "working directory and environment",
            "No implicit shell",
            "stdin is closed",
            "BEFORE",
            "rec test npm --help",
            "300",
            "124",
            "127",
            "128 + signal",
            "4 KiB",
            "two Steps",
            "Do not retry automatically",
            "set -e",
            "stop_rc",
            "test_rc",
        ],
    );
}

#[test]
fn stop_help_does_not_promise_automatic_recovery_or_ui_verification() {
    let s = Sandbox::new();
    let help = s.help(&["stop", "--help"]);
    contains_all(
        &help,
        &[
            "Requires no active test",
            "NOT the truth",
            "raw data",
            "not all capture failures",
            "No resume or recovery",
            "second stop",
            "Never auto-upload",
        ],
    );
}

#[test]
fn usage_errors_also_do_not_touch_the_session() {
    let s = Sandbox::new();
    for args in [
        vec!["start", "--app", "Safari", "--window-id", "1"],
        vec!["test", "--timeout-secs", "0", "cargo", "test"],
        vec!["observe", "--status", "verified", "no"],
        vec!["note"],
        vec!["--json"],
    ] {
        let result = s.invoke(&args);
        assert_eq!(
            result.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    s.assert_untouched();
}

#[test]
fn skill_metadata_and_references_form_a_portable_package() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills/agent-work-recorder");
    let skill = fs::read_to_string(root.join("SKILL.md")).unwrap();
    let mut parts = skill.splitn(3, "---\n");
    assert_eq!(parts.next(), Some(""));
    let frontmatter = parts.next().unwrap();
    let fields: std::collections::BTreeMap<_, _> = frontmatter
        .lines()
        .map(|l| l.split_once(": ").expect("flat skill metadata"))
        .collect();
    assert_eq!(fields["name"], "agent-work-recorder");
    assert!(fields["description"].chars().count() <= 1024 && !fields["description"].is_empty());
    assert!(fields["compatibility"].chars().count() <= 500);
    assert_eq!(
        fields.len(),
        3,
        "do not silently add tool auto-authorization"
    );
    assert!(skill.lines().count() < 500);
    for path in ["references/workflows.md", "references/troubleshooting.md"] {
        assert!(skill.contains(&format!("({path})")));
        assert!(root.join(path).is_file());
    }
    contains_all(
        &skill,
        &[
            "installed CLI",
            "does not authorize",
            "Only continue if start succeeds",
            "same user, HOME and TMPDIR",
            "owning-app audio",
            "Do not blindly rerun",
            "actual video",
            "synthetic",
            "new Run",
            "Human acceptance",
        ],
    );
}
