mod collector;
mod input_keys;
mod input_trace;
mod network;
mod protocol;
mod state;
mod web;

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use protocol::{HistoryRequest, NodeMetadata};
use state::TelemetryStore;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "Desktop telemetry node for watchmedo")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    Serve(ServeArgs),
    Watch(WatchArgs),
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

#[derive(Debug, Clone, Args)]
struct ServeArgs {
    #[arg(long, default_value = "/ip4/0.0.0.0/tcp/4100")]
    listen: String,

    #[arg(long, default_value = "127.0.0.1:8080")]
    web_listen: SocketAddr,

    #[arg(long, default_value_t = 1_000)]
    sample_interval_ms: u64,

    #[arg(long, default_value_t = 60)]
    retention_secs: u64,

    #[arg(long, default_value_t = 600)]
    max_samples: usize,

    #[arg(long, default_value_t = 8)]
    top_processes: usize,

    #[arg(
        long,
        conflicts_with_all = ["input_trace_preset", "input_trace_program", "input_trace_arg"],
        help = "Unix socket path for a privileged keyboard trace sidecar",
        long_help = "Unix socket path for a privileged keyboard trace sidecar. This is the safer deployment option because watchmedo serve can stay unprivileged while a separate sidecar process with root or caps connects and forwards raw input_event trace lines over IPC."
    )]
    input_trace_socket: Option<PathBuf>,

    #[arg(
        long,
        value_enum,
        conflicts_with_all = ["input_trace_program", "input_trace_arg"],
        help = "Built-in keyboard trace preset, for example bpftrace-input-event",
        long_help = "Built-in keyboard trace preset that expands to a known executable plus arguments. Use this instead of --input-trace-program/--input-trace-arg when you want the standard input_event bpftrace command."
    )]
    input_trace_preset: Option<InputTracePreset>,

    #[arg(
        long,
        help = "Executable that emits Linux input events on stdout, for example bpftrace",
        long_help = "Executable that emits Linux input events on stdout. This is the program name, for example 'bpftrace', not a raw probe string like 'kprobe:input_event'. The command's stdout must contain lines with 'dev_ptr=... type=... code=... value=...'."
    )]
    input_trace_program: Option<String>,

    #[arg(
        long,
        allow_hyphen_values = true,
        requires = "input_trace_program",
        help = "Argument to pass to --input-trace-program; repeat for multiple args",
        long_help = "Argument to pass to --input-trace-program. Repeat this flag for multiple arguments. Example: --input-trace-program bpftrace --input-trace-arg -e --input-trace-arg 'kprobe:input_event { printf(\"dev_ptr=%p type=%u code=%u value=%d\\n\", arg0, arg1, arg2, arg3); }'"
    )]
    input_trace_arg: Vec<String>,

    #[arg(long, default_value_t = 2048, help = "Bounded queue capacity for raw input events before decode")]
    input_trace_buffer: usize,

    #[arg(long, default_value_t = 128, help = "Maximum decoded keyboard events to flush in one batch")]
    input_trace_batch_size: usize,

    #[arg(long, default_value_t = 25, help = "Flush interval in milliseconds for keyboard batches")]
    input_trace_flush_ms: u64,

    #[arg(long, default_value_t = 1_000, help = "Restart delay in milliseconds if the trace command exits or fails")]
    input_trace_restart_backoff_ms: u64,

    #[arg(long, help = "Filter to a specific input device pointer; repeat to allow multiple devices")]
    input_trace_device_ptr: Vec<String>,

    #[arg(long, value_enum, help = "Filter key states emitted to the API; repeat to allow multiple of up, down, repeat")]
    input_trace_state: Vec<input_trace::KeyboardTraceStateFilter>,
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, InputTracePreset};
    use clap::Parser;

    #[test]
    fn serve_accepts_hyphen_prefixed_input_trace_args() {
        let cli = Cli::try_parse_from([
            "watchmedo",
            "serve",
            "--input-trace-program",
            "bpftrace",
            "--input-trace-arg",
            "-e",
            "--input-trace-arg",
            "kprobe:input_event { printf(\"dev_ptr=%p type=%u code=%u value=%d\\n\", arg0, arg1, arg2, arg3); }",
        ])
        .expect("serve args should parse");

        let Command::Serve(args) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(args.input_trace_program.as_deref(), Some("bpftrace"));
        assert_eq!(
            args.input_trace_arg,
            vec![
                "-e",
                "kprobe:input_event { printf(\"dev_ptr=%p type=%u code=%u value=%d\\n\", arg0, arg1, arg2, arg3); }",
            ]
        );
    }

    #[test]
    fn serve_accepts_input_trace_preset() {
        let cli = Cli::try_parse_from(["watchmedo", "serve", "--input-trace-preset", "bpftrace-input-event"])
            .expect("serve preset should parse");

        let Command::Serve(args) = cli.command else {
            panic!("expected serve command");
        };

        assert!(matches!(args.input_trace_preset, Some(InputTracePreset::BpftraceInputEvent)));
    }

    #[test]
    fn bpftrace_input_event_preset_expands_to_expected_command() {
        let (program, args) = InputTracePreset::BpftraceInputEvent.command();

        assert_eq!(program, "bpftrace");
        assert_eq!(args, vec![
            "-e".to_owned(),
            "kprobe:input_event { printf(\"dev_ptr=%p type=%u code=%u value=%d\\n\", arg0, arg1, arg2, arg3); }".to_owned(),
        ]);
    }

    #[test]
    fn serve_accepts_input_trace_socket() {
        let cli = Cli::try_parse_from([
            "watchmedo",
            "serve",
            "--input-trace-socket",
            "/tmp/watchmedo-input.sock",
        ])
        .expect("serve socket args should parse");

        let Command::Serve(args) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(
            args.input_trace_socket.as_deref(),
            Some(std::path::Path::new("/tmp/watchmedo-input.sock"))
        );
    }
}

#[derive(Debug, Clone, Args)]
struct WatchArgs {
    #[arg(long)]
    connect: String,

    #[arg(long)]
    lookback_secs: Option<u64>,

    #[arg(long, default_value_t = 8)]
    max_processes: usize,

    #[arg(long, default_value_t = false)]
    no_processes: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("watchmedo=info,libp2p_swarm=warn")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve(args) => {
            let sample_interval = Duration::from_millis(args.sample_interval_ms);
            let retention = Duration::from_secs(args.retention_secs);

            let state = Arc::new(RwLock::new(TelemetryStore::new(
                NodeMetadata::capture(),
                retention,
                sample_interval,
                args.max_samples,
            )));

            collector::spawn_collector(
                Arc::clone(&state),
                collector::CollectorConfig {
                    interval: sample_interval,
                    top_processes: args.top_processes,
                },
            )
            .context("failed to start telemetry collector")?;

            let keyboard_trace = if let Some(socket_path) = args.input_trace_socket.clone() {
                Some(input_trace::spawn_keyboard_trace_socket_service(
                    Arc::clone(&state),
                    input_trace::KeyboardTraceSocketConfig {
                        socket_path,
                        channel_capacity: args.input_trace_buffer,
                        max_batch_size: args.input_trace_batch_size,
                        flush_interval: Duration::from_millis(args.input_trace_flush_ms),
                        restart_backoff: Duration::from_millis(args.input_trace_restart_backoff_ms),
                        filter: input_trace::KeyboardTraceFilter {
                            device_ptrs: args.input_trace_device_ptr.clone(),
                            states: args.input_trace_state.clone(),
                        },
                    },
                )
                .context("failed to start keyboard input trace socket service")?)
            } else if let Some((program, trace_args)) = args
                .input_trace_preset
                .map(InputTracePreset::command)
                .or_else(|| {
                    args.input_trace_program
                        .clone()
                        .map(|program| (program, args.input_trace_arg.clone()))
                })
            {
                Some(input_trace::spawn_keyboard_trace_service(
                    Arc::clone(&state),
                    input_trace::KeyboardTraceConfig {
                        program,
                        args: trace_args,
                        channel_capacity: args.input_trace_buffer,
                        max_batch_size: args.input_trace_batch_size,
                        flush_interval: Duration::from_millis(args.input_trace_flush_ms),
                        restart_backoff: Duration::from_millis(args.input_trace_restart_backoff_ms),
                        filter: input_trace::KeyboardTraceFilter {
                            device_ptrs: args.input_trace_device_ptr.clone(),
                            states: args.input_trace_state.clone(),
                        },
                    },
                )
                .context("failed to start keyboard input trace service")?)
            } else {
                None
            };

            let mut p2p_task = tokio::spawn(network::run_server(
                Arc::clone(&state),
                network::NetworkConfig {
                    listen_addr: args.listen.parse().context("invalid listen multiaddr")?,
                },
            ));
            let mut web_task = tokio::spawn(web::run(
                Arc::clone(&state),
                web::WebConfig {
                    listen_addr: args.web_listen,
                    keyboard_trace,
                },
            ));

            tokio::select! {
                result = &mut p2p_task => {
                    web_task.abort();
                    let p2p_result = result.context("p2p server task panicked")?;
                    let _ = web_task.await;
                    p2p_result
                }
                result = &mut web_task => {
                    p2p_task.abort();
                    let web_result = result.context("web api task panicked")?;
                    let _ = p2p_task.await;
                    web_result
                }
            }
        }
        Command::Watch(args) => {
            network::run_client(network::ClientConfig {
                connect_addr: args.connect.parse().context("invalid connect multiaddr")?,
                history_request: HistoryRequest {
                    lookback_secs: args.lookback_secs,
                    include_processes: !args.no_processes,
                    max_processes: Some(args.max_processes),
                },
            })
            .await
        }
    }
}
