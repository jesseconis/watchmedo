use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::Command,
    time,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "Privileged keyboard trace sidecar for watchmedo")]
struct Cli {
    #[arg(
        long,
        help = "Unix socket path exposed by 'watchmedo serve --input-trace-socket ...'",
        long_help = "Unix socket path exposed by 'watchmedo serve --input-trace-socket ...'. This sidecar connects to that socket and forwards raw input_event trace lines so the main watchmedo process can remain unprivileged."
    )]
    socket_path: PathBuf,

    #[arg(
        long,
        value_enum,
        conflicts_with_all = ["trace_program", "trace_arg"],
        help = "Built-in trace preset, defaults to bpftrace-input-event when no manual command is provided"
    )]
    trace_preset: Option<InputTracePreset>,

    #[arg(long, help = "Trace executable to spawn, for example bpftrace")]
    trace_program: Option<String>,

    #[arg(long, allow_hyphen_values = true, requires = "trace_program", help = "Argument to pass to --trace-program; repeat for multiple args")]
    trace_arg: Vec<String>,

    #[arg(long, default_value_t = 1_000, help = "Restart delay in milliseconds if the socket or trace command fails")]
    restart_backoff_ms: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum InputTracePreset {
    BpftraceInputEvent,
}

impl InputTracePreset {
    fn command(self) -> (String, Vec<String>) {
        match self {
            Self::BpftraceInputEvent => (
                "bpftrace".to_owned(),
                vec![
                    "-e".to_owned(),
                    "kprobe:input_event { printf(\"dev_ptr=%p type=%u code=%u value=%d\\n\", arg0, arg1, arg2, arg3); }"
                        .to_owned(),
                ],
            ),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("watchmedo_input_sidecar=info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let (program, args) = resolve_trace_command(&cli);
    let restart_backoff = Duration::from_millis(cli.restart_backoff_ms.max(1));

    info!(socket = %cli.socket_path.display(), %program, args = ?args, "input sidecar started");

    loop {
        match run_sidecar_once(&cli.socket_path, &program, &args).await {
            Ok(status) => {
                warn!(?status, "input sidecar trace command exited; restarting after backoff");
            }
            Err(error) => {
                warn!(?error, "input sidecar loop failed; restarting after backoff");
            }
        }

        time::sleep(restart_backoff).await;
    }
}

fn resolve_trace_command(cli: &Cli) -> (String, Vec<String>) {
    cli.trace_preset
        .unwrap_or(InputTracePreset::BpftraceInputEvent)
        .command()
        .pipe_if(cli.trace_program.is_some(), |_| {
            (
                cli.trace_program.clone().expect("trace_program presence checked"),
                cli.trace_arg.clone(),
            )
        })
}

async fn run_sidecar_once(
    socket_path: &Path,
    program: &str,
    args: &[String],
) -> anyhow::Result<std::process::ExitStatus> {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect to watchmedo input socket {}", socket_path.display()))?;

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn trace program {}", program))?;

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(line, "input sidecar stderr");
            }
        });
    }

    let stdout = child
        .stdout
        .take()
        .context("trace program did not provide stdout")?;
    let mut lines = BufReader::new(stdout).lines();

    while let Some(line) = lines.next_line().await.context("failed reading trace program stdout")? {
        if let Err(error) = stream.write_all(line.as_bytes()).await {
            let _ = child.kill().await;
            return Err(error).with_context(|| format!("failed writing to watchmedo input socket {}", socket_path.display()));
        }

        if let Err(error) = stream.write_all(b"\n").await {
            let _ = child.kill().await;
            return Err(error).with_context(|| format!("failed writing newline to watchmedo input socket {}", socket_path.display()));
        }
    }

    child.wait().await.context("failed to wait on trace program")
}

trait PipeIf: Sized {
    fn pipe_if<F>(self, condition: bool, f: F) -> Self
    where
        F: FnOnce(Self) -> Self;
}

impl<T> PipeIf for T {
    fn pipe_if<F>(self, condition: bool, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        if condition {
            f(self)
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, InputTracePreset, resolve_trace_command};
    use clap::Parser;

    #[test]
    fn sidecar_accepts_socket_path_and_preset() {
        let cli = Cli::try_parse_from([
            "watchmedo-input-sidecar",
            "--socket-path",
            "/tmp/watchmedo-input.sock",
            "--trace-preset",
            "bpftrace-input-event",
        ])
        .expect("sidecar args should parse");

        assert_eq!(cli.socket_path, std::path::PathBuf::from("/tmp/watchmedo-input.sock"));
        assert!(matches!(cli.trace_preset, Some(InputTracePreset::BpftraceInputEvent)));
    }

    #[test]
    fn sidecar_defaults_to_bpftrace_preset() {
        let cli = Cli::try_parse_from([
            "watchmedo-input-sidecar",
            "--socket-path",
            "/tmp/watchmedo-input.sock",
        ])
        .expect("sidecar args should parse");

        let (program, args) = resolve_trace_command(&cli);
        assert_eq!(program, "bpftrace");
        assert_eq!(args[0], "-e");
        assert!(args[1].contains("kprobe:input_event"));
    }
}