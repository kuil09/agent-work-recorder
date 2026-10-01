//! MP4 finalization. Never report success after silently dropping audio or chapters.
use crate::protocol::{EventKind, RunEvent};
use anyhow::{ensure, Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub start_ms: u64,
    pub end_ms: u64,
    pub title: String,
}

pub fn chapters(events: &[RunEvent], duration_ms: u64) -> Vec<Chapter> {
    if duration_ms == 0 {
        return Vec::new();
    }
    let mut starts = vec![(0, "Setup".to_string())];
    for e in events {
        if matches!(e.kind, EventKind::Checkpoint | EventKind::TestStart)
            && e.media_ms < duration_ms
        {
            let prefix = if e.kind == EventKind::TestStart {
                "Test"
            } else {
                "Checkpoint"
            };
            starts.push((
                e.media_ms,
                format!(
                    "{prefix} {} — {}",
                    e.step_id,
                    crate::runner::display_text(&e.text, 180)
                ),
            ));
        }
    }
    starts.sort_by_key(|s| s.0);
    let mut unique: Vec<(u64, String)> = Vec::new();
    for (time, title) in starts {
        if let Some(last) = unique.last_mut() {
            if last.0 == time {
                last.1 = title;
                continue;
            }
        }
        unique.push((time, title));
    }
    unique
        .iter()
        .enumerate()
        .map(|(i, (start, title))| Chapter {
            start_ms: *start,
            end_ms: unique.get(i + 1).map(|v| v.0).unwrap_or(duration_ms),
            title: title.clone(),
        })
        .collect()
}

pub fn escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' | '=' | ';' | '#' | '\n' => {
                out.push('\\');
                out.push(c);
            }
            '\r' | '\0' => {}
            _ => out.push(c),
        }
    }
    out
}

pub fn metadata(
    title: Option<&str>,
    run_id: &str,
    commit: Option<&str>,
    created_at: &str,
    chapters: &[Chapter],
) -> String {
    let mut out = String::from(";FFMETADATA1\n");
    if let Some(title) = title {
        out.push_str(&format!("title={}\n\n", escape(title)));
    }
    out.push_str(&format!("comment=runId={}\n", escape(run_id)));
    if let Some(commit) = commit {
        out.push_str(&format!("synopsis=git {}\n", escape(commit)));
    }
    if !created_at.is_empty() {
        out.push_str(&format!("creation_time={}\n", escape(created_at)));
    }
    // Only numeric boundaries go through the ffmetadata chapter parser. In FFmpeg,
    // a doubled trailing backslash can still absorb the newline as a continuation.
    // Exact user titles are supplied as separate argv metadata values in remux().
    for (index, c) in chapters.iter().enumerate() {
        out.push_str(&format!(
            "\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle=Chapter {}\n",
            c.start_ms,
            c.end_ms,
            index + 1
        ));
    }
    out
}

/// Prefer the package's private media tools without changing child-test PATH.
fn media_tool(name: &str) -> PathBuf {
    if let Ok(executable) = std::env::current_exe().and_then(fs::canonicalize) {
        if let Some(directory) = executable.parent() {
            let bundled = directory.join(name);
            if fs::metadata(&bundled)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                return bundled;
            }
        }
    }
    PathBuf::from(name)
}

pub fn preflight() -> Result<()> {
    for bin in ["ffmpeg", "ffprobe"] {
        let output = Command::new(media_tool(bin))
            .arg("-version")
            .output()
            .with_context(|| format!("{bin} is required beside rec or on PATH"))?;
        ensure!(output.status.success(), "{bin} is not usable");
    }
    Ok(())
}

pub fn probe(path: &Path) -> Result<serde_json::Value> {
    let output = Command::new(media_tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-show_chapters",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .context("ffprobe")?;
    ensure!(
        output.status.success(),
        "invalid MP4: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).context("invalid ffprobe response")
}

pub fn duration_ms(probe: &serde_json::Value) -> Result<u64> {
    let seconds: f64 = probe["format"]["duration"]
        .as_str()
        .context("video duration missing")?
        .parse()?;
    ensure!(
        seconds.is_finite() && seconds > 0.0,
        "capture has no duration"
    );
    Ok((seconds * 1000.0).round().max(1.0) as u64)
}

pub fn validate(probe: &serde_json::Value, audio: bool) -> Result<()> {
    let streams = probe["streams"]
        .as_array()
        .context("capture streams missing")?;
    ensure!(
        streams
            .iter()
            .any(|s| s["codec_type"] == "video" && s["codec_name"] == "h264"),
        "capture has no H.264 video"
    );
    if audio {
        ensure!(
            streams
                .iter()
                .any(|s| s["codec_type"] == "audio" && s["codec_name"] == "aac"),
            "system audio was requested, but capture has no AAC audio; refusing silent success"
        );
    } else {
        ensure!(
            !streams.iter().any(|s| s["codec_type"] == "audio"),
            "unexpected audio in a video-only recording"
        );
    }
    duration_ms(probe)?;
    Ok(())
}

pub fn remux(
    raw: &Path,
    output: &Path,
    metadata_path: &Path,
    expected_chapters: &[Chapter],
    audio: bool,
    run_id: &str,
) -> Result<()> {
    ensure!(
        !output.exists(),
        "output already exists: {}",
        output.display()
    );
    let name = output
        .file_name()
        .context("output must be a file")?
        .to_string_lossy();
    let temp = output.with_file_name(format!(".{name}.{run_id}.partial.mp4"));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        let mut command = Command::new(media_tool("ffmpeg"));
        command
            .args(["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(raw)
            .args(["-f", "ffmetadata", "-i"])
            .arg(metadata_path)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                "-map_metadata",
                "1",
                "-map_chapters",
                "1",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
            ]);
        for (index, chapter) in expected_chapters.iter().enumerate() {
            command
                .arg(format!("-metadata:c:{index}"))
                .arg(format!("title={}", chapter.title));
        }
        let status = command.arg(&temp).output().context("ffmpeg remux")?;
        ensure!(
            status.status.success(),
            "ffmpeg remux failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let info = probe(&temp)?;
        validate(&info, audio)?;
        let actual = info["chapters"]
            .as_array()
            .context("final MP4 chapters missing")?;
        ensure!(
            actual.len() == expected_chapters.len(),
            "chapter count mismatch"
        );
        for (a, e) in actual.iter().zip(expected_chapters) {
            let start: f64 = a["start_time"]
                .as_str()
                .context("chapter time missing")?
                .parse()?;
            let end: f64 = a["end_time"]
                .as_str()
                .context("chapter end missing")?
                .parse()?;
            ensure!(
                (start * 1000.0 - e.start_ms as f64).abs() <= 2.0
                    && (end * 1000.0 - e.end_ms as f64).abs() <= 2.0,
                "chapter timestamps did not survive muxing"
            );
            ensure!(
                a["tags"]["title"].as_str() == Some(e.title.as_str()),
                "chapter title did not survive muxing: expected {:?}, got {:?}",
                e.title,
                a["tags"]["title"]
            );
        }
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        fs::hard_link(&temp, output).context("publish MP4 without overwriting an existing file")?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: EventKind, ms: u64) -> RunEvent {
        RunEvent {
            ts_ms: 0,
            media_ms: ms,
            step: 1,
            step_id: "7F32:001".into(),
            kind,
            text: "한글 # = ; \\".into(),
            status: None,
            test_result: None,
        }
    }
    #[test]
    fn chapters_are_major_events_only_and_clamped() {
        let c = chapters(
            &[
                event(EventKind::Note, 10),
                event(EventKind::TestStart, 100),
                event(EventKind::Checkpoint, 200),
                event(EventKind::Observe, 250),
                event(EventKind::Checkpoint, 9000),
            ],
            500,
        );
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].end_ms, 100);
        assert_eq!(c[2].end_ms, 500);
        assert!(chapters(&[], 0).is_empty());
        assert_eq!(chapters(&[event(EventKind::TestStart, 0)], 100).len(), 1);
    }
    #[test]
    fn metadata_escapes_injection_and_preserves_unicode() {
        assert_eq!(escape("한글=#;\\\n"), "한글\\=\\#\\;\\\\\\\n");
        let m = metadata(Some("title\n[CHAPTER]"), "ABCD", None, "", &[]);
        assert!(m.contains("title=title\\\n[CHAPTER]"));
    }
    #[test]
    fn missing_requested_audio_is_an_error() {
        let p = serde_json::json!({"streams":[{"codec_type":"video","codec_name":"h264"}], "format":{"duration":"1.0"}});
        assert!(validate(&p, true).is_err());
        assert!(validate(&p, false).is_ok());
    }
    #[test]
    fn ffmpeg_roundtrip_preserves_audio_and_chapters() {
        preflight().expect("install ffmpeg/ffprobe to run media tests");
        let dir = std::env::temp_dir().join(format!("rec-media-{}", crate::id::generate_run_id()));
        fs::create_dir(&dir).unwrap();
        let raw = dir.join("raw.mp4");
        let output = dir.join("result space.mp4");
        let meta = dir.join("chapters.txt");
        let s = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=320x240:rate=30:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&raw)
            .status()
            .unwrap();
        assert!(s.success());
        let cs = chapters(
            &[event(EventKind::Checkpoint, 500)],
            duration_ms(&probe(&raw).unwrap()).unwrap(),
        );
        fs::write(
            &meta,
            metadata(
                Some("한글 # ="),
                "7F32",
                Some("abc1234"),
                "2026-09-30T00:00:00Z",
                &cs,
            ),
        )
        .unwrap();
        remux(&raw, &output, &meta, &cs, true, "7F32").unwrap();
        assert!(remux(&raw, &output, &meta, &cs, true, "7F32").is_err());
        let p = probe(&output).unwrap();
        validate(&p, true).unwrap();
        assert_eq!(p["format"]["tags"]["title"], "한글 # =");
        assert_eq!(p["chapters"].as_array().unwrap().len(), 2);
        assert!(p["chapters"][1]["tags"]["title"]
            .as_str()
            .unwrap()
            .ends_with('\\'));
        fs::remove_dir_all(dir).unwrap();
    }
}
