use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};

use anyhow::{Context, bail};
use clap::Parser;
use glob::glob;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
    process::Command,
    sync::mpsc,
    task::JoinHandle,
    time,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use watchmedo_probe::{
    InputTracePreset, ModuleState, ModuleStatus, ProbeEventPayload, ProbeModuleConfig,
    ProbeModuleId, SIDECAR_PROTOCOL_VERSION, ServerMessage, ShellTracePreset,
    SidecarMessage, parse_bpftrace_input_event_line, parse_bpftrace_shell_command_line,
    read_frame, write_frame,
};

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "Privileged probe sidecar for watchmedo")]
struct Cli {
    #[arg(
        long,
        help = "Unix socket path exposed by 'watchmedo serve --input-trace-socket ...'",
        long_help = "Unix socket path exposed by 'watchmedo serve --input-trace-socket ...'. The sidecar connects to this socket, receives the runtime probe configuration from watchmedo serve, and streams structured probe events back over the same IPC channel."
    )]
    socket_path: PathBuf,

    #[arg(long, default_value_t = 1_000, help = "Restart delay in milliseconds if the socket or probe runtime fails")]
    restart_backoff_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("watchmedo_sidecar=info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let restart_backoff = Duration::from_millis(cli.restart_backoff_ms.max(1));

    info!(socket = %cli.socket_path.display(), "probe sidecar started");

    loop {
        match run_sidecar_once(&cli.socket_path).await {
            Ok(()) => warn!("probe sidecar disconnected; restarting after backoff"),
            Err(error) => warn!(?error, "probe sidecar failed; restarting after backoff"),
        }

        time::sleep(restart_backoff).await;
    }
}

type ProbeTaskHandle = JoinHandle<anyhow::Result<()>>;

trait SidecarProbeModule: Send {
    fn id(&self) -> ProbeModuleId;

    fn spawn(self: Box<Self>, sender: mpsc::Sender<SidecarMessage>) -> ProbeTaskHandle;
}

struct KeyboardInputModule {
    program: String,
    args: Vec<String>,
}

impl KeyboardInputModule {
    fn new() -> Self {
        let (program, args) = InputTracePreset::BpftraceInputEvent.command();
        Self { program, args }
    }

    async fn run(self, sender: mpsc::Sender<SidecarMessage>) -> anyhow::Result<()> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn keyboard probe command {}", self.program))?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(line, "keyboard probe stderr");
                }
            });
        }

        let stdout = child
            .stdout
            .take()
            .context("keyboard probe command did not provide stdout")?;
        let mut lines = BufReader::new(stdout).lines();

        while let Some(line) = lines.next_line().await.context("failed reading keyboard probe stdout")? {
            let payload = match parse_bpftrace_input_event_line(&line) {
                Ok(Some(payload)) => payload,
                Ok(None) => continue,
                Err(reason) => {
                    warn!(reason, %line, "failed to parse keyboard probe line");
                    continue;
                }
            };

            sender
                .send(SidecarMessage::event(
                    ProbeModuleId::KeyboardInput,
                    "raw_input",
                    ProbeEventPayload::KeyboardRawInput(payload),
                ))
                .await
                .context("watchmedo serve closed the sidecar event channel")?;
        }

        let status = child.wait().await.context("failed to wait on keyboard probe command")?;
        if !status.success() {
            bail!("keyboard probe command exited with status {status}");
        }

        Ok(())
    }
}

impl SidecarProbeModule for KeyboardInputModule {
    fn id(&self) -> ProbeModuleId {
        ProbeModuleId::KeyboardInput
    }

    fn spawn(self: Box<Self>, sender: mpsc::Sender<SidecarMessage>) -> ProbeTaskHandle {
        tokio::spawn(async move { self.run(sender).await })
    }
}

struct ShellCommandModule {
    program: String,
    args: Vec<String>,
}

impl ShellCommandModule {
    fn new() -> anyhow::Result<Self> {
        let zsh_module_path = discover_zsh_zle_module_path().context("failed to locate zsh zle module for shell probe")?;
        let (program, args) = ShellTracePreset::BpftraceZshZleread.program(&zsh_module_path);

        Ok(Self { program, args })
    }

    async fn run(self, sender: mpsc::Sender<SidecarMessage>) -> anyhow::Result<()> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn shell probe command {}", self.program))?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(line, "shell probe stderr");
                }
            });
        }

        let stdout = child
            .stdout
            .take()
            .context("shell probe command did not provide stdout")?;
        let mut lines = BufReader::new(stdout).lines();

        while let Some(line) = lines.next_line().await.context("failed reading shell probe stdout")? {
            let payload = match parse_bpftrace_shell_command_line(&line) {
                Ok(Some(payload)) => payload,
                Ok(None) => continue,
                Err(reason) => {
                    warn!(reason, %line, "failed to parse shell probe line");
                    continue;
                }
            };

            sender
                .send(SidecarMessage::event(
                    ProbeModuleId::ShellCommands,
                    "command",
                    ProbeEventPayload::ShellCommand(payload),
                ))
                .await
                .context("watchmedo serve closed the sidecar event channel")?;
        }

        let status = child.wait().await.context("failed to wait on shell probe command")?;
        if !status.success() {
            bail!("shell probe command exited with status {status}");
        }

        Ok(())
    }
}

impl SidecarProbeModule for ShellCommandModule {
    fn id(&self) -> ProbeModuleId {
        ProbeModuleId::ShellCommands
    }

    fn spawn(self: Box<Self>, sender: mpsc::Sender<SidecarMessage>) -> ProbeTaskHandle {
        tokio::spawn(async move { self.run(sender).await })
    }
}

async fn run_sidecar_once(socket_path: &Path) -> anyhow::Result<()> {
    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect to watchmedo input socket {}", socket_path.display()))?;

    let (mut reader, mut writer) = stream.into_split();
    let server_message = read_frame::<_, ServerMessage>(&mut reader)
        .await
        .with_context(|| format!("failed to read sidecar configuration from {}", socket_path.display()))?
        .context("watchmedo serve closed the socket before sending a configuration")?;

    let requested_modules = match server_message {
        ServerMessage::Configure {
            protocol_version,
            modules,
        } => {
            if protocol_version != SIDECAR_PROTOCOL_VERSION {
                bail!(
                    "unsupported sidecar protocol version {protocol_version}, expected {}",
                    SIDECAR_PROTOCOL_VERSION
                );
            }

            modules
        }
        ServerMessage::Shutdown { .. } => bail!("watchmedo serve requested shutdown before starting probes"),
    };

    let (modules, mut statuses) = build_modules(requested_modules);
    if modules.is_empty() {
        bail!("watchmedo serve did not request any supported probes");
    }

    statuses.extend(modules.iter().map(|module| ModuleStatus {
        module: module.id(),
        state: ModuleState::Running,
        detail: None,
    }));

    write_frame(
        &mut writer,
        &SidecarMessage::Status {
            protocol_version: SIDECAR_PROTOCOL_VERSION,
            modules: statuses,
        },
    )
    .await
    .context("failed to send sidecar status")?;

    let (sender, mut receiver) = mpsc::channel(256);
    let writer_task = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            write_frame(&mut writer, &message)
                .await
                .context("failed to write sidecar message")?;
        }

        Ok::<(), anyhow::Error>(())
    });

    let mut tasks = Vec::with_capacity(modules.len());
    for module in modules {
        tasks.push(module.spawn(sender.clone()));
    }
    drop(sender);

    let mut first_error = None;
    for task in tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(anyhow::Error::new(error).context("sidecar probe task panicked"));
                }
            }
        }
    }

    writer_task
        .await
        .context("sidecar writer task panicked")??;

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(())
}

fn build_modules(
    requested_modules: Vec<ProbeModuleConfig>,
) -> (Vec<Box<dyn SidecarProbeModule>>, Vec<ModuleStatus>) {
    let mut runnable_modules: Vec<Box<dyn SidecarProbeModule>> = Vec::new();
    let mut statuses = Vec::new();

    for config in requested_modules {
        match config.module {
            ProbeModuleId::KeyboardInput => runnable_modules.push(Box::new(KeyboardInputModule::new())),
            ProbeModuleId::DnsQueries => statuses.push(ModuleStatus {
                module: ProbeModuleId::DnsQueries,
                state: ModuleState::Unsupported,
                detail: Some("dns sidecar probe is not implemented yet".to_owned()),
            }),
            ProbeModuleId::ShellCommands => match ShellCommandModule::new() {
                Ok(module) => runnable_modules.push(Box::new(module)),
                Err(error) => statuses.push(ModuleStatus {
                    module: ProbeModuleId::ShellCommands,
                    state: ModuleState::Failed,
                    detail: Some(error.to_string()),
                }),
            },
        }
    }

    (runnable_modules, statuses)
}

fn discover_zsh_zle_module_path() -> anyhow::Result<String> {
    const PATTERNS: &[&str] = &[
        "/usr/lib/zsh/*/zsh/zle.so",
        "/usr/lib64/zsh/*/zsh/zle.so",
        "/usr/local/lib/zsh/*/zsh/zle.so",
    ];

    for pattern in PATTERNS {
        let mut matches = glob(pattern)
            .with_context(|| format!("invalid zsh module glob pattern {pattern}"))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        matches.sort();

        if let Some(path) = matches.into_iter().next_back() {
            return Ok(path.display().to_string());
        }
    }

    bail!("could not find zsh zle.so in standard library locations")
}

#[cfg(test)]
mod tests {
    use super::build_modules;
    use super::discover_zsh_zle_module_path;
    use watchmedo_probe::{ModuleState, ProbeModuleConfig, ProbeModuleId};

    #[test]
    fn keeps_keyboard_runnable_and_marks_future_modules_unsupported() {
        let (modules, statuses) = build_modules(vec![
            ProbeModuleConfig::enabled(ProbeModuleId::KeyboardInput),
            ProbeModuleConfig::enabled(ProbeModuleId::DnsQueries),
        ]);

        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].id(), ProbeModuleId::KeyboardInput);
        assert!(statuses.iter().any(|status| {
            status.module == ProbeModuleId::DnsQueries && status.state == ModuleState::Unsupported
        }));
    }

    #[test]
    fn shell_module_is_runnable_or_returns_failure_status() {
        let (modules, statuses) = build_modules(vec![ProbeModuleConfig::enabled(ProbeModuleId::ShellCommands)]);

        assert!(modules.iter().all(|module| module.id() == ProbeModuleId::ShellCommands));
        assert!(modules.len() + statuses.len() >= 1);
    }

    #[test]
    fn zsh_discovery_returns_result_or_not_found() {
        let result = discover_zsh_zle_module_path();
        if let Ok(path) = result {
            assert!(path.ends_with("/zle.so"));
        }
    }
}