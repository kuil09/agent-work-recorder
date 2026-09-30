use crate::protocol::CaptureHud;
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct CuaGrabber {
    stop: Arc<AtomicBool>,
    hud: Arc<Mutex<serde_json::Value>>,
    worker: Option<JoinHandle<Result<u32>>>,
    frames_dir: PathBuf,
    raw_path: PathBuf,
}

#[derive(Clone)]
struct CuaTarget {
    bin: PathBuf,
    socket: PathBuf,
}

fn discover_cua() -> Result<CuaTarget> {
    if let (Ok(bin), Ok(sock)) = (
        std::env::var("REC_CUA_BIN"),
        std::env::var("REC_CUA_SOCKET"),
    ) {
        return Ok(CuaTarget {
            bin: PathBuf::from(bin),
            socket: PathBuf::from(sock),
        });
    }
    let out = Command::new("ps")
        .args(["-ax", "-o", "args="])
        .output()
        .context("ps")?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if !line.contains("cua-driver") || !line.contains("serve") || !line.contains("--socket") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(bin) = parts.first().map(PathBuf::from) else {
            continue;
        };
        if let Some(i) = parts.iter().position(|p| *p == "--socket") {
            if let Some(sock) = parts.get(i + 1) {
                let socket = PathBuf::from(sock);
                if socket.exists() && bin.exists() {
                    return Ok(CuaTarget { bin, socket });
                }
            }
        }
    }
    bail!("no CuaDriver socket (Screen Recording fallback unavailable)")
}

fn cua_call(target: &CuaTarget, tool: &str, args: &serde_json::Value) -> Result<String> {
    let out = Command::new(&target.bin)
        .arg("--socket")
        .arg(&target.socket)
        .arg("call")
        .arg(tool)
        .arg(args.to_string())
        .output()
        .context("cua-driver call")?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        bail!("cua-driver {tool} failed: {stderr} {stdout}");
    }
    Ok(stdout)
}

impl CuaGrabber {
    pub fn start(
        run_id: &str,
        tmp_dir: &Path,
        raw_path: PathBuf,
        capture_bin: &Path,
        initial: CaptureHud,
    ) -> Result<Self> {
        let target = discover_cua()?;
        let frames_dir = tmp_dir.join("frames");
        fs::create_dir_all(&frames_dir)?;
        let hud_path = tmp_dir.join("hud.json");
        let mut initial_json = serde_json::to_value(&initial)?;
        if let Some(obj) = initial_json.as_object_mut() {
            obj.insert("run_id".into(), json!(run_id));
            obj.insert("show_git".into(), json!(true));
            obj.insert("show_card".into(), json!(false));
        }
        fs::write(&hud_path, serde_json::to_vec_pretty(&initial_json)?)?;

        let probe = tmp_dir.join("probe.png");
        let resp = cua_call(
            &target,
            "get_desktop_state",
            &json!({ "screenshot_out_file": probe.display().to_string() }),
        )?;
        if !probe.exists() {
            bail!("CuaDriver screenshot failed: {resp}");
        }

        let stop = Arc::new(AtomicBool::new(false));
        let hud = Arc::new(Mutex::new(initial_json));
        let stop2 = stop.clone();
        let hud2 = hud.clone();
        let frames_dir2 = frames_dir.clone();
        let hud_path2 = hud_path.clone();
        let capture_bin = capture_bin.to_path_buf();
        let run_id = run_id.to_string();
        let target2 = target.clone();

        let worker = thread::spawn(move || {
            let mut n: u32 = 0;
            let started = Instant::now();
            while !stop2.load(Ordering::SeqCst) {
                n += 1;
                let raw = frames_dir2.join(format!("raw-{n:05}.png"));
                let stamped = frames_dir2.join(format!("f-{n:05}.png"));
                match cua_call(
                    &target2,
                    "get_desktop_state",
                    &json!({ "screenshot_out_file": raw.display().to_string() }),
                ) {
                    Ok(_) if raw.exists() => {
                        let state = hud2.lock().unwrap().clone();
                        let mut obj = state;
                        if let Some(map) = obj.as_object_mut() {
                            map.insert("run_id".into(), json!(run_id));
                            if started.elapsed() < Duration::from_secs(5) {
                                map.insert("show_git".into(), json!(true));
                            } else {
                                map.insert("show_git".into(), json!(false));
                            }
                        }
                        let _ = fs::write(&hud_path2, serde_json::to_vec(&obj).unwrap_or_default());
                        let st = Command::new(&capture_bin)
                            .args([
                                "stamp",
                                "--input",
                                raw.to_str().unwrap_or(""),
                                "--output",
                                stamped.to_str().unwrap_or(""),
                                "--state",
                                hud_path2.to_str().unwrap_or(""),
                                "--run-id",
                                &run_id,
                            ])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                        if !matches!(st, Ok(s) if s.success()) {
                            let _ = fs::copy(&raw, &stamped);
                        }
                        let _ = fs::remove_file(&raw);
                    }
                    Ok(resp) => eprintln!("cua grab: {resp}"),
                    Err(e) => eprintln!("cua grab: {e:#}"),
                }
                thread::sleep(Duration::from_millis(450));
            }
            Ok(n)
        });

        eprintln!("capture: {{\"event\":\"ready\",\"backend\":\"cua\"}}");
        Ok(CuaGrabber {
            stop,
            hud,
            worker: Some(worker),
            frames_dir,
            raw_path,
        })
    }

    pub fn send(&self, hud: &CaptureHud) -> Result<()> {
        let incoming = serde_json::to_value(hud)?;
        let mut cur = self.hud.lock().unwrap();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), incoming.as_object()) {
            for (k, v) in src {
                if !v.is_null() {
                    dst.insert(k.clone(), v.clone());
                }
            }
        } else {
            *cur = incoming;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        let frames = if let Some(h) = self.worker.take() {
            h.join().unwrap_or(Ok(0))?
        } else {
            0
        };
        if frames == 0 {
            bail!("CuaDriver grabber captured 0 frames");
        }
        let list = self.frames_dir.join("list.txt");
        let mut f = File::create(&list)?;
        let mut names: Vec<_> = fs::read_dir(&self.frames_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("f-") && n.ends_with(".png"))
                    .unwrap_or(false)
            })
            .collect();
        names.sort();
        for p in &names {
            writeln!(f, "file '{}'", p.display())?;
            writeln!(f, "duration 0.5")?;
        }
        if let Some(last) = names.last() {
            writeln!(f, "file '{}'", last.display())?;
        }
        f.flush()?;

        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(&list)
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&self.raw_path)
            .status()
            .context("ffmpeg concat")?;
        if !status.success() {
            bail!("ffmpeg concat failed");
        }
        Ok(())
    }
}
