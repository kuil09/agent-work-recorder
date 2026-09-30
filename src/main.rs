use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rec::protocol::{IpcRequest, IpcResponse, ObserveStatus};
use rec::session::{self, SessionFile};
use rec::{daemon, git, id};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "rec",
    about = "Agent Work Recorder — record agent work as a reviewable MP4",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a recording run
    Start {
        #[arg(long)]
        title: Option<String>,
        /// Capture target: `full` for the main display
        #[arg(long)]
        screen: Option<String>,
        /// Window title or application name (substring match)
        #[arg(long)]
        window: Option<String>,
        /// CGWindowID
        #[arg(long)]
        window_id: Option<u32>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Record current action / context (creates a Step)
    Note { text: Vec<String> },
    /// Record an expected result (creates a Step)
    Expect { text: Vec<String> },
    /// Record an agent observation / claim (creates a Step)
    Observe {
        #[arg(long, value_enum)]
        status: Option<StatusArg>,
        text: Vec<String>,
    },
    /// Mark a human-review checkpoint (creates a Step)
    Checkpoint { text: Vec<String> },
    /// Stop recording and write the MP4
    Stop,
    /// Internal: recorder daemon
    #[command(hide = true)]
    Daemon {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        workdir: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        screen: Option<String>,
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        window_id: Option<u32>,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum StatusArg {
    Pass,
    Fail,
    Uncertain,
    Info,
}

impl From<StatusArg> for ObserveStatus {
    fn from(value: StatusArg) -> Self {
        match value {
            StatusArg::Pass => ObserveStatus::Pass,
            StatusArg::Fail => ObserveStatus::Fail,
            StatusArg::Uncertain => ObserveStatus::Uncertain,
            StatusArg::Info => ObserveStatus::Info,
        }
    }
}

fn join_text(text: Vec<String>) -> Result<String> {
    let s = text.join(" ").trim().to_string();
    if s.is_empty() {
        bail!("text is required");
    }
    Ok(s)
}

fn send_ipc(req: IpcRequest) -> Result<IpcResponse> {
    let session = session::require_active_session()?;
    let mut stream = UnixStream::connect(&session.socket)
        .with_context(|| format!("connect {}", session.socket))?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    serde_json::to_writer(&mut stream, &req)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    let line = buf.lines().next().unwrap_or(&buf);
    let resp: IpcResponse = serde_json::from_str(line).context("invalid daemon response")?;
    Ok(resp)
}

fn print_resp(resp: IpcResponse) -> Result<()> {
    if !resp.ok {
        bail!("{}", resp.error.unwrap_or_else(|| "request failed".into()));
    }
    if let (Some(step_id), Some(kind)) = (resp.step_id.as_ref(), resp.kind.as_ref()) {
        println!("[{step_id}] {kind} recorded");
    }
    if let Some(v) = resp.verdict {
        println!("{v}");
    }
    Ok(())
}

fn start_run(
    title: Option<String>,
    screen: Option<String>,
    window: Option<String>,
    window_id: Option<u32>,
    output: Option<PathBuf>,
) -> Result<()> {
    if let Some(existing) = session::load_session()? {
        eprintln!("An active recording already exists.\n");
        eprintln!("Run: {}", existing.run_id);
        std::process::exit(1);
    }

    let targets = [screen.is_some(), window.is_some(), window_id.is_some()]
        .into_iter()
        .filter(|b| *b)
        .count();
    if targets > 1 {
        bail!("use only one of --screen, --window, --window-id");
    }
    let screen = match (screen, window.is_some(), window_id.is_some()) {
        (Some(s), false, false) => Some(s),
        (None, false, false) => Some("full".into()),
        (s, _, _) => s,
    };

    let workdir = std::env::current_dir()?;
    let run_id = id::generate_run_id();
    let filename = id::output_filename(&run_id, &title);
    let output = match output {
        Some(p) => {
            if p.is_absolute() {
                p
            } else {
                workdir.join(p)
            }
        }
        None => session::recordings_dir(&workdir).join(filename),
    };

    let exe = std::env::current_exe()?;
    let log_dir = session::session_dir()?.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{run_id}.log"));
    let log = std::fs::File::create(&log_path)?;

    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .arg("--run-id")
        .arg(&run_id)
        .arg("--workdir")
        .arg(&workdir)
        .arg("--output")
        .arg(&output);
    if let Some(t) = &title {
        cmd.arg("--title").arg(t);
    }
    if let Some(s) = &screen {
        cmd.arg("--screen").arg(s);
    }
    if let Some(w) = &window {
        cmd.arg("--window").arg(w);
    }
    if let Some(id) = window_id {
        cmd.arg("--window-id").arg(id.to_string());
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("spawn recorder daemon")?;

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(session) = session::load_session()? {
            if session.run_id == run_id {
                print_start_banner(&session, &title)?;
                return Ok(());
            }
        }
        if Instant::now() > deadline {
            let tail = log_text
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            bail!("recorder daemon failed to start\n{tail}");
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

fn print_start_banner(session: &SessionFile, title: &Option<String>) -> Result<()> {
    let git = git::GitInfo::collect(std::path::Path::new(&session.workdir));
    println!("REC ●\n");
    println!("Run        {}", session.run_id);
    if let Some(t) = title {
        println!("Title      {t}");
    }
    if git.available {
        println!("Repository {}", git.repository.as_deref().unwrap_or("-"));
        println!("Branch     {}", git.branch.as_deref().unwrap_or("-"));
        println!("Commit     {}", git.commit.as_deref().unwrap_or("-"));
        if git.working_tree.as_deref() == Some("DIRTY") {
            println!("Working tree: dirty");
        }
    } else {
        println!("Git: unavailable");
    }
    println!("Output     {}", session.output);
    Ok(())
}

fn stop_run() -> Result<()> {
    let resp = send_ipc(IpcRequest::Stop)?;
    if !resp.ok {
        bail!("{}", resp.error.unwrap_or_else(|| "stop failed".into()));
    }
    println!("Recording complete\n");
    if let Some(d) = resp.duration {
        println!("Duration   {d}");
    }
    if let Some(s) = resp.steps {
        println!("Steps      {s}");
    }
    if let Some(c) = resp.checkpoints {
        println!("Checkpoints {c}");
    }
    println!();
    if let Some(o) = resp.output {
        println!("{o}");
    }
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Start {
            title,
            screen,
            window,
            window_id,
            output,
        } => start_run(title, screen, window, window_id, output),
        Commands::Note { text } => print_resp(send_ipc(IpcRequest::Note {
            text: join_text(text)?,
        })?),
        Commands::Expect { text } => print_resp(send_ipc(IpcRequest::Expect {
            text: join_text(text)?,
        })?),
        Commands::Observe { status, text } => {
            let resp = send_ipc(IpcRequest::Observe {
                text: join_text(text)?,
                status: status.map(Into::into),
            })?;
            print_resp(resp)
        }
        Commands::Checkpoint { text } => print_resp(send_ipc(IpcRequest::Checkpoint {
            text: join_text(text)?,
        })?),
        Commands::Stop => stop_run(),
        Commands::Daemon {
            run_id,
            title,
            workdir,
            output,
            screen,
            window,
            window_id,
        } => daemon::run(daemon::DaemonArgs {
            run_id,
            title,
            workdir,
            output,
            screen,
            window,
            window_id,
        }),
    }
}
