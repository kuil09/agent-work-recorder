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

mod help;

#[derive(Parser, Debug)]
#[command(
    name = "rec",
    about = "Record agent work and claims as a reviewable MP4",
    long_about = help::OVERVIEW,
    after_help = "Use rec <COMMAND> --help for contracts, examples and failure handling.",
    after_long_help = help::OVERVIEW_DETAILS,
    version
)]
struct Cli { #[command(subcommand)] command: Commands }

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start one local recording run
    #[command(long_about = help::START, after_long_help = help::START_DETAILS)]
    Start {
        #[arg(long, value_name = "TEXT", help = "Title for the Run, initial context and MP4 metadata",
            long_help = "Human-readable purpose of this Run. Used in initial context, MP4 title and the default filename slug. Quote spaces. Does not select a capture target or execute a task.")]
        title: Option<String>,
        #[command(flatten)] capture: CaptureOptions,
        #[arg(long, value_name = "FILE.mp4", help = "Final MP4 path; must not already exist",
            long_help = "Final output must end in .mp4. Relative paths are resolved from rec start's working directory. Defaults to ./recordings/<RUNID>-<title-slug>.mp4, or <RUNID>.mp4 without a title. Existing files are never overwritten; stop publishes the final file.")]
        output: Option<PathBuf>,
    },
    /// Record current action/context
    #[command(long_about = help::NOTE, after_long_help = help::NOTE_DETAILS)]
    Note { #[arg(required = true, value_name = "TEXT", help = "Nonblank action/context; quote as one short statement")] text: Vec<String> },
    /// Record expected results, not a verdict
    #[command(long_about = help::EXPECT, after_long_help = help::EXPECT_DETAILS)]
    Expect { #[arg(required = true, value_name = "TEXT", help = "Observable acceptance criterion, written before the check")] text: Vec<String> },
    /// Record an agent observation/claim
    #[command(long_about = help::OBSERVE, after_long_help = help::OBSERVE_DETAILS)]
    Observe {
        #[arg(long, value_enum, value_name = "VERDICT", help = "Agent claim: pass, fail, uncertain or info (default: info)",
            long_help = "Agent's claimed verdict, not a recorder assertion. Omission behaves as info and clears any previous verdict. Use uncertain when evidence is insufficient; never infer pass solely from a command's exit code.")]
        status: Option<StatusArg>,
        #[arg(required = true, value_name = "TEXT", help = "What was actually observed, including evidence or uncertainty")]
        text: Vec<String>,
    },
    /// Create a review checkpoint and MP4 chapter boundary
    #[command(long_about = help::CHECKPOINT, after_long_help = help::CHECKPOINT_DETAILS)]
    Checkpoint { #[arg(required = true, value_name = "TEXT", help = "Scene label for human review and the MP4 chapter")] text: Vec<String> },
    /// Run a noninteractive command; preserve its exit code, never infer PASS
    #[command(long_about = help::TEST, after_long_help = help::TEST_DETAILS)]
    Test {
        /// Terminate the test process group after this many seconds
        #[arg(long, value_name = "SECONDS", default_value_t = 300,
            long_help = "Positive timeout in seconds. Put this option before COMMAND. On timeout the test process group is terminated and the wrapper returns 124; the recording stays active and still needs stop.",
            value_parser = clap::value_parser!(u64).range(1..))]
        timeout_secs: u64,
        #[arg(required = true, value_name = "COMMAND", num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true,
            help = "Executable followed by its arguments; no implicit shell",
            long_help = "Executable plus argv, forwarded without an implicit shell. Uses this caller's cwd/environment with closed stdin. Tokens after the executable belong to the child. Use rec test --help for recorder help, or rec test -- PROGRAM --help to run the program's help.")]
        command: Vec<String>,
    },
    /// Finalize a validated MP4 with chapters and optional audio
    #[command(long_about = help::STOP, after_long_help = help::STOP_DETAILS)]
    Stop,
    #[command(hide = true)]
    Daemon { #[arg(long)] config: PathBuf },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum StatusArg {
    /// Agent claims the observed result matches the stated criterion
    Pass,
    /// Agent claims the observed result does not match the criterion
    Fail,
    /// Available evidence does not justify a pass/fail claim
    Uncertain,
    /// Observation without a verdict; clears the previous verdict
    Info,
}
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

    #[test]
    fn help_contract_builds_and_examples_parse_without_execution() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        for input in [
            vec!["rec", "start", "--title", "Login review", "--app", "com.apple.Safari"],
            vec!["rec", "start", "--window", "Login - Google Chrome", "--output", "./review.mp4"],
            vec!["rec", "start", "--window-id", "12345"],
            vec!["rec", "start", "--screen", "full", "--system-audio"],
            vec!["rec", "note", "--", "--compact is being investigated"],
            vec!["rec", "expect", "로그인 실패 시 오류가 표시된다."],
            vec!["rec", "observe", "--status", "uncertain", "Result unconfirmed"],
            vec!["rec", "checkpoint", "Final review scene"],
            vec!["rec", "test", "--", "sh", "-c", "printf 'diagnostic\\n'; exit 7"],
            vec!["rec", "stop"],
        ] {
            Cli::try_parse_from(&input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
        }
    }

    #[test]
    fn test_help_and_child_help_have_distinct_argv_boundaries() {
        use clap::error::ErrorKind;
        assert_eq!(Cli::try_parse_from(["rec", "test", "--help"]).unwrap_err().kind(), ErrorKind::DisplayHelp);
        for input in [
            vec!["rec", "test", "--timeout-secs", "60", "--", "npm", "--help", "--timeout-secs", "1"],
            vec!["rec", "test", "--timeout-secs", "60", "npm", "--help", "--timeout-secs", "1"],
        ] {
            let cli = Cli::try_parse_from(input).unwrap();
            match cli.command {
                Commands::Test { timeout_secs, command } => {
                    assert_eq!(timeout_secs, 60);
                    assert_eq!(command, ["npm", "--help", "--timeout-secs", "1"]);
                }
                _ => panic!("expected test"),
            }
        }
    }
}
