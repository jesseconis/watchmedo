use std::io;

use clap::ValueEnum;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const SIDECAR_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "kebab-case")]
pub enum ProbeModuleId {
    KeyboardInput,
    DnsQueries,
    ShellCommands,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeModuleConfig {
    pub module: ProbeModuleId,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
}

impl ProbeModuleConfig {
    pub fn enabled(module: ProbeModuleId) -> Self {
        Self {
            module,
            params: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum ServerMessage {
    Configure {
        protocol_version: u32,
        modules: Vec<ProbeModuleConfig>,
    },
    Shutdown {
        protocol_version: u32,
    },
}

impl ServerMessage {
    pub fn configure(modules: Vec<ProbeModuleConfig>) -> Self {
        Self::Configure {
            protocol_version: SIDECAR_PROTOCOL_VERSION,
            modules,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleState {
    Starting,
    Running,
    Stopped,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleStatus {
    pub module: ProbeModuleId,
    pub state: ModuleState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardRawEventPayload {
    pub device_ptr: Option<String>,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl KeyboardRawEventPayload {
    pub const fn new(device_ptr: Option<String>, event_type: u16, code: u16, value: i32) -> Self {
        Self {
            device_ptr,
            event_type,
            code,
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandPayload {
    pub pid: u32,
    pub uid: u32,
    pub command: String,
    pub executable: Option<String>,
}

impl ShellCommandPayload {
    pub fn new(pid: u32, uid: u32, command: impl Into<String>) -> Self {
        let command = command.into();

        Self {
            executable: shell_executable(&command),
            pid,
            uid,
            command,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "payload_type", content = "data", rename_all = "snake_case")]
pub enum ProbeEventPayload {
    KeyboardRawInput(KeyboardRawEventPayload),
    ShellCommand(ShellCommandPayload),
    Json(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeEvent {
    pub module: ProbeModuleId,
    pub event_name: String,
    pub captured_at_ms: u64,
    pub payload: ProbeEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum SidecarMessage {
    Status {
        protocol_version: u32,
        modules: Vec<ModuleStatus>,
    },
    Event {
        protocol_version: u32,
        event: ProbeEvent,
    },
}

impl SidecarMessage {
    pub fn event(module: ProbeModuleId, event_name: impl Into<String>, payload: ProbeEventPayload) -> Self {
        Self::Event {
            protocol_version: SIDECAR_PROTOCOL_VERSION,
            event: ProbeEvent {
                module,
                event_name: event_name.into(),
                captured_at_ms: now_ms(),
                payload,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum InputTracePreset {
    BpftraceInputEvent,
}

impl InputTracePreset {
    pub fn command(self) -> (String, Vec<String>) {
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShellTracePreset {
    BpftraceZshZleread,
}

impl ShellTracePreset {
    pub fn program(self, zsh_module_path: &str) -> (String, Vec<String>) {
        match self {
            Self::BpftraceZshZleread => (
                "bpftrace".to_owned(),
                vec![
                    "-e".to_owned(),
                    format!(
                        "uretprobe:{zsh_module_path}:zleread {{ printf(\"pid=%d uid=%d cmd=%s\\n\", pid, uid, str(retval)); }}"
                    ),
                ],
            ),
        }
    }
}

pub fn parse_bpftrace_input_event_line(line: &str) -> Result<Option<KeyboardRawEventPayload>, &'static str> {
    let Some(marker) = line.find("dev_ptr=") else {
        return Ok(None);
    };

    let payload = &line[marker..];
    let device_ptr = parse_field(payload, "dev_ptr")?.to_owned();
    let event_type = parse_field(payload, "type")?
        .parse::<u16>()
        .map_err(|_| "invalid type field")?;
    let code = parse_field(payload, "code")?
        .parse::<u16>()
        .map_err(|_| "invalid code field")?;
    let value = parse_field(payload, "value")?
        .parse::<i32>()
        .map_err(|_| "invalid value field")?;

    Ok(Some(KeyboardRawEventPayload::new(
        Some(device_ptr),
        event_type,
        code,
        value,
    )))
}

pub fn parse_bpftrace_shell_command_line(line: &str) -> Result<Option<ShellCommandPayload>, &'static str> {
    let Some(pid_start) = line.find("pid=") else {
        return Ok(None);
    };

    let payload = &line[pid_start..];
    let pid = parse_field(payload, "pid")?
        .parse::<u32>()
        .map_err(|_| "invalid pid field")?;
    let uid = parse_field(payload, "uid")?
        .parse::<u32>()
        .map_err(|_| "invalid uid field")?;

    let command = if let Some(command) = parse_remainder_field(payload, "cmd=") {
        command
    } else if let Some(command) = parse_remainder_field(payload, "cmd:") {
        command
    } else {
        return Err("missing command field");
    };

    if command.is_empty() {
        return Err("empty command field");
    }

    Ok(Some(ShellCommandPayload::new(pid, uid, command)))
}

pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(message).map_err(invalid_data)?;
    let length = u32::try_from(payload.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "message too large"))?;

    writer.write_u32_le(length).await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = match reader.read_u32_le().await {
        Ok(length) => length,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut payload = vec![0_u8; length as usize];
    reader.read_exact(&mut payload).await?;

    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(invalid_data)
}

fn parse_field<'a>(payload: &'a str, key: &str) -> Result<&'a str, &'static str> {
    let token = format!("{key}=");
    let start = payload.find(&token).ok_or("missing field")? + token.len();
    let rest = &payload[start..];

    rest.split_whitespace().next().ok_or("missing field value")
}

fn parse_remainder_field<'a>(payload: &'a str, token: &str) -> Option<&'a str> {
    let start = payload.find(token)? + token.len();
    Some(payload[start..].trim())
}

fn shell_executable(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn invalid_data(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        InputTracePreset, KeyboardRawEventPayload, ProbeModuleConfig, ProbeModuleId,
        ServerMessage, ShellCommandPayload, ShellTracePreset,
        parse_bpftrace_input_event_line, parse_bpftrace_shell_command_line,
    };

    #[test]
    fn keyboard_preset_expands_to_expected_command() {
        let (program, args) = InputTracePreset::BpftraceInputEvent.command();

        assert_eq!(program, "bpftrace");
        assert_eq!(args[0], "-e");
        assert!(args[1].contains("kprobe:input_event"));
    }

    #[test]
    fn parses_bpftrace_input_event_lines() {
        let parsed = parse_bpftrace_input_event_line("dev_ptr=0xffff894a62670000 type=1 code=20 value=1")
            .expect("expected valid parse result")
            .expect("expected parsed payload");

        assert_eq!(
            parsed,
            KeyboardRawEventPayload::new(Some("0xffff894a62670000".to_owned()), 1, 20, 1)
        );
    }

    #[test]
    fn ignores_non_matching_lines() {
        assert!(parse_bpftrace_input_event_line("Attached 1 probe")
            .expect("expected ignored line")
            .is_none());
    }

    #[test]
    fn config_message_carries_modules() {
        let message = ServerMessage::configure(vec![ProbeModuleConfig::enabled(ProbeModuleId::KeyboardInput)]);
        let json = serde_json::to_string(&message).expect("message should serialize");

        assert!(json.contains("keyboard_input"));
        assert!(json.contains("configure"));
    }

    #[test]
    fn shell_preset_expands_to_expected_command() {
        let (program, args) = ShellTracePreset::BpftraceZshZleread.program("/usr/lib/zsh/5.9/zsh/zle.so");

        assert_eq!(program, "bpftrace");
        assert_eq!(args[0], "-e");
        assert!(args[1].contains("zleread"));
    }

    #[test]
    fn parses_bpftrace_shell_command_lines() {
        let parsed = parse_bpftrace_shell_command_line("pid=133582 uid=1000 cmd=echo hello")
            .expect("expected valid parse result")
            .expect("expected parsed payload");

        assert_eq!(
            parsed,
            ShellCommandPayload::new(133582, 1000, "echo hello")
        );
        assert_eq!(parsed.executable.as_deref(), Some("echo"));
    }

    #[test]
    fn parses_colon_form_shell_command_lines() {
        let parsed = parse_bpftrace_shell_command_line("01:32:49 pid=133582 uid=1000 cmd: date +%s")
            .expect("expected valid parse result")
            .expect("expected parsed payload");

        assert_eq!(parsed.command, "date +%s");
        assert_eq!(parsed.executable.as_deref(), Some("date"));
    }
}