use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rec::daemon::{CaptureOptions, DaemonArgs};
use rec::protocol::{IpcRequest, IpcResponse, ObserveStatus};
use rec::session::{self, SessionFile};
use rec::{daemon, git, id, media, runner};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "rec", about = "Record agent work and claims as a reviewable MP4", version)]
struct Cli { #[command(subcommand)] command: Commands }

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start one local recording run
    Start {
        #[arg(long)] title: Option<String>,
        #[command(flatten)] capture: CaptureOptions,
        #[arg(long)] output: Option<PathBuf>,
    },
    /// Record current action/context
    Note { #[arg(required = true)] text: Vec<String> },
    /// Record expected results, not a verdict
    Expect { #[arg(required = true)] text: Vec<String> },
    /// Record an agent observation/claim
    Observe { #[arg(long, value_enum)] status: Option<StatusArg>, #[arg(required = true)] text: Vec<String> },
    /// Create a review checkpoint and MP4 chapter boundary
    Checkpoint { #[arg(required = true)] text: Vec<String> },
    /// Run a noninteractive command; preserve its exit code, never infer PASS
    Test {
        /// Terminate the test process group after this many seconds
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
        timeout_secs: u64,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Finalize a validated MP4 with chapters and optional audio
    Stop,
    #[command(hide = true)]
    Daemon { #[arg(long)] config: PathBuf },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum StatusArg { Pass, Fail, Uncertain, Info }
impl From<StatusArg> for ObserveStatus {
    fn from(value: StatusArg) -> Self {
        match value { StatusArg::Pass => Self::Pass, StatusArg::Fail => Self::Fail, StatusArg::Uncertain => Self::Uncertain, StatusArg::Info => Self::Info }
    }
}
fn join_text(text: Vec<String>) -> Result<String> {
    let text = text.join(" ").trim().to_string();
    ensure!(!text.is_empty(), "text is required"); Ok(text)
}
fn send_to(session: &SessionFile, req: IpcRequest) -> Result<IpcResponse> {
    let mut stream = UnixStream::connect(&session.socket).context("connect recorder daemon")?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    serde_json::to_writer(&mut stream, &req)?; stream.write_all(b"\n")?; stream.flush()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let resp: IpcResponse = serde_json::from_str(&reply).context("invalid daemon response")?;
    ensure!(resp.ok, "{}", resp.error.as_deref().unwrap_or("request failed"));
    Ok(resp)
}
fn send_ipc(req: IpcRequest) -> Result<IpcResponse> { send_to(&session::require_active_session()?, req) }
fn print_resp(resp: &IpcResponse) {
    if let (Some(step), Some(kind)) = (&resp.step_id, &resp.kind) { println!("[{step}] {kind} recorded"); }
    if let Some(verdict) = &resp.verdict { println!("{verdict}"); }
}
fn log_tail(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_default().lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn start_run(title: Option<String>, capture: CaptureOptions, output: Option<PathBuf>) -> Result<()> {
    capture.validate()?;
    if let Some(existing) = session::load_session()? { bail!("An active recording already exists.\nRun: {}", existing.run_id); }
    media::preflight()?;
    let workdir = std::env::current_dir()?;
    let mut selected = None;
    for _ in 0..64 {
        let run_id = id::generate_run_id();
        let path = session::run_tmp_dir(&run_id);
        if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
        match fs::create_dir(&path) {
            Ok(()) => { fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?; selected = Some((run_id, path)); break; }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let (run_id, tmp) = selected.context("could not reserve a run ID")?;
    let output = output.map(|p| if p.is_absolute() { p } else { workdir.join(p) })
        .unwrap_or_else(|| session::recordings_dir(&workdir).join(id::output_filename(&run_id, &title)));
    ensure!(output.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("mp4")) == Some(true), "--output must have an .mp4 extension");
    ensure!(!output.exists(), "output already exists: {}", output.display());
    let config = tmp.join("args.json");
    let args = DaemonArgs { run_id: run_id.clone(), title: title.clone(), workdir, output, capture };
    id::write_json_atomic(&config, &args)?;
    let log_dir = session::session_dir()?.join("logs"); fs::create_dir_all(&log_dir)?;
    fs::set_permissions(session::session_dir()?, fs::Permissions::from_mode(0o700))?;
    let log_path = log_dir.join(format!("{run_id}.log")); let log = fs::File::create(&log_path)?;
    let mut child = Command::new(std::env::current_exe()?).arg("daemon").arg("--config").arg(&config)
        .stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn().context("spawn recorder daemon")?;
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(session) = session::load_session()? {
            if session.run_id == run_id {
                println!("REC\n\nRun        {run_id}\nTarget     {}\nAudio      {}", args.capture.describe(), if args.capture.system_audio { "system audio (no microphone)" } else { "OFF" });
                if let Some(t) = &title { println!("Title      {t}"); }
                let git = git::GitInfo::collect(std::path::Path::new(&session.workdir));
                if git.available {
                    println!("Repository {}\nBranch     {}\nCommit     {}\nWorking tree {}", git.repository.as_deref().unwrap_or("-"), git.branch.as_deref().unwrap_or("-"), git.commit.as_deref().unwrap_or("-"), git.working_tree.as_deref().unwrap_or("-"));
                } else { println!("Git: unavailable"); }
                println!("Output     {}", session.output); return Ok(());
            }
        }
        if let Some(status) = child.try_wait()? { bail!("recorder daemon exited during startup ({status})\n{}", log_tail(&log_path)); }
        if Instant::now() >= deadline {
            let _ = child.kill(); let _ = child.wait();
            bail!("recorder daemon failed to start\n{}", log_tail(&log_path));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn test_run(command: Vec<String>, timeout_secs: u64) -> Result<i32> {
    // Bind both messages to the SAME socket/run, even if the global session changes.
    let session = session::require_active_session()?;
    runner::install_signal_handlers();
    let begin = send_to(&session, IpcRequest::TestBegin { command: command.clone(), cwd: std::env::current_dir()?.display().to_string(), owner_pid: std::process::id() })?;
    let step = begin.step.context("test start response has no step")?;
    print_resp(&begin);
    let result = runner::execute(&command, Duration::from_secs(timeout_secs), true);
    let code = result.cli_exit_code();
    eprintln!("\nCommand exit: {code}; duration: {:.3}s", result.duration_ms as f64 / 1000.0);
    let end = send_to(&session, IpcRequest::TestEnd { run_id: session.run_id.clone(), test_step: step, result })
        .context("command finished but its result could not be recorded")?;
    print_resp(&end); Ok(code)
}

fn run() -> Result<i32> {
    match Cli::parse().command {
        Commands::Start { title, capture, output } => start_run(title, capture, output)?,
        Commands::Note { text } => print_resp(&send_ipc(IpcRequest::Note { text: join_text(text)? })?),
        Commands::Expect { text } => print_resp(&send_ipc(IpcRequest::Expect { text: join_text(text)? })?),
        Commands::Observe { text, status } => print_resp(&send_ipc(IpcRequest::Observe { text: join_text(text)?, status: status.map(Into::into) })?),
        Commands::Checkpoint { text } => print_resp(&send_ipc(IpcRequest::Checkpoint { text: join_text(text)? })?),
        Commands::Test { command, timeout_secs } => return test_run(command, timeout_secs),
        Commands::Stop => {
            let r = send_ipc(IpcRequest::Stop)?;
            println!("Recording complete\n\nDuration   {}\nSteps      {}\nCheckpoints {}\nTests      {}\n\n{}", r.duration.unwrap_or_default(), r.steps.unwrap_or(0), r.checkpoints.unwrap_or(0), r.tests.unwrap_or(0), r.output.unwrap_or_default());
        }
        Commands::Daemon { config } => daemon::run(serde_json::from_slice(&fs::read(config)?)?)?,
    }
    Ok(0)
}
fn main() {
    match run() { Ok(code) => std::process::exit(code), Err(e) => { eprintln!("{e:#}"); std::process::exit(1); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_arguments_are_forwarded_verbatim() {
        for input in [vec!["rec", "test", "npm", "test", "--", "auth.test.ts"], vec!["rec", "test", "--", "npm", "test", "--", "auth.test.ts"]] {
            let cli = Cli::try_parse_from(input).unwrap();
            if let Commands::Test { command, .. } = cli.command { assert_eq!(command, ["npm", "test", "--", "auth.test.ts"]); } else { panic!("expected test"); }
        }
    }
    #[test]
    fn app_and_audio_options_parse_without_scope_expansion() {
        let cli = Cli::try_parse_from(["rec", "start", "--app", "com.apple.Safari", "--system-audio"]).unwrap();
        if let Commands::Start { capture, .. } = cli.command { assert!(capture.system_audio); assert!(capture.validate().is_ok()); } else { panic!("expected start"); }
        assert!(Cli::try_parse_from(["rec", "start", "--app", "Safari", "--window-id", "1"]).is_err());
    }
}
