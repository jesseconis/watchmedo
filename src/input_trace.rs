use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use clap::ValueEnum;
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixListener,
    process::Command,
    sync::mpsc::error::TrySendError,
    sync::{RwLock, mpsc},
    time::{self, MissedTickBehavior},
};
use tracing::{debug, info, warn};

use crate::{
    input_keys::{KeyState, KeyboardEventDecoder, KeyboardKeyEvent, PendingKeyboardEvent, RawInputEvent},
    state::TelemetryStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardTraceStateFilter {
    Up,
    Down,
    Repeat,
}

impl KeyboardTraceStateFilter {
    fn matches(self, state: KeyState) -> bool {
        matches!(
            (self, state),
            (Self::Up, KeyState::Up)
                | (Self::Down, KeyState::Down)
                | (Self::Repeat, KeyState::Repeat)
        )
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct KeyboardTraceFilter {
    pub device_ptrs: Vec<String>,
    pub states: Vec<KeyboardTraceStateFilter>,
}

impl KeyboardTraceFilter {
    pub fn matches_device(&self, device_ptr: Option<&str>) -> bool {
        self.device_ptrs.is_empty()
            || device_ptr.is_some_and(|candidate| self.device_ptrs.iter().any(|allowed| allowed == candidate))
    }

    pub fn matches_key(&self, key: &KeyboardKeyEvent) -> bool {
        self.states.is_empty() || self.states.iter().any(|filter| filter.matches(key.state))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardTraceReaderState {
    Starting,
    Running,
    Backoff,
    Stopped,
}

impl Default for KeyboardTraceReaderState {
    fn default() -> Self {
        Self::Stopped
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyboardTraceStatusSnapshot {
    pub enabled: bool,
    pub source_kind: &'static str,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub socket_path: Option<String>,
    pub reader_state: KeyboardTraceReaderState,
    pub queue_capacity: usize,
    pub queue_depth: usize,
    pub max_batch_size: usize,
    pub flush_interval_ms: u64,
    pub restart_backoff_ms: u64,
    pub restart_count: u64,
    pub total_lines_seen: u64,
    pub total_non_event_lines: u64,
    pub total_parse_errors: u64,
    pub total_filtered_events: u64,
    pub total_enqueued_events: u64,
    pub total_published_events: u64,
    pub total_batches_flushed: u64,
    pub total_dropped_events: u64,
    pub last_started_at_ms: Option<u64>,
    pub last_event_at_ms: Option<u64>,
    pub last_exit_at_ms: Option<u64>,
    pub last_exit_status: Option<String>,
    pub last_error: Option<String>,
    pub filter: KeyboardTraceFilter,
}

impl KeyboardTraceStatusSnapshot {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            source_kind: "disabled",
            program: None,
            args: Vec::new(),
            socket_path: None,
            reader_state: KeyboardTraceReaderState::Stopped,
            queue_capacity: 0,
            queue_depth: 0,
            max_batch_size: 0,
            flush_interval_ms: 0,
            restart_backoff_ms: 0,
            restart_count: 0,
            total_lines_seen: 0,
            total_non_event_lines: 0,
            total_parse_errors: 0,
            total_filtered_events: 0,
            total_enqueued_events: 0,
            total_published_events: 0,
            total_batches_flushed: 0,
            total_dropped_events: 0,
            last_started_at_ms: None,
            last_event_at_ms: None,
            last_exit_at_ms: None,
            last_exit_status: None,
            last_error: None,
            filter: KeyboardTraceFilter::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyboardTraceConfig {
    pub program: String,
    pub args: Vec<String>,
    pub channel_capacity: usize,
    pub max_batch_size: usize,
    pub flush_interval: Duration,
    pub restart_backoff: Duration,
    pub filter: KeyboardTraceFilter,
}

#[derive(Debug, Clone)]
pub struct KeyboardTraceSocketConfig {
    pub socket_path: PathBuf,
    pub channel_capacity: usize,
    pub max_batch_size: usize,
    pub flush_interval: Duration,
    pub restart_backoff: Duration,
    pub filter: KeyboardTraceFilter,
}

#[derive(Debug, Clone)]
struct KeyboardTraceProcessingConfig {
    max_batch_size: usize,
    flush_interval: Duration,
    filter: KeyboardTraceFilter,
}

#[derive(Debug, Clone)]
pub struct KeyboardTraceHandle {
    runtime: Arc<KeyboardTraceRuntime>,
}

impl KeyboardTraceHandle {
    pub async fn snapshot(&self) -> KeyboardTraceStatusSnapshot {
        self.runtime.snapshot().await
    }
}

#[derive(Debug, Default)]
struct KeyboardTraceRuntimeShared {
    reader_state: KeyboardTraceReaderState,
    last_exit_status: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug)]
struct KeyboardTraceRuntime {
    source_kind: &'static str,
    program: Option<String>,
    args: Vec<String>,
    socket_path: Option<String>,
    queue_capacity: usize,
    max_batch_size: usize,
    flush_interval: Duration,
    restart_backoff: Duration,
    filter: KeyboardTraceFilter,
    queue_depth: AtomicUsize,
    total_lines_seen: AtomicU64,
    total_non_event_lines: AtomicU64,
    total_parse_errors: AtomicU64,
    total_filtered_events: AtomicU64,
    total_enqueued_events: AtomicU64,
    total_published_events: AtomicU64,
    total_batches_flushed: AtomicU64,
    total_dropped_events: AtomicU64,
    restart_count: AtomicU64,
    last_started_at_ms: AtomicU64,
    last_event_at_ms: AtomicU64,
    last_exit_at_ms: AtomicU64,
    shared: RwLock<KeyboardTraceRuntimeShared>,
}

impl KeyboardTraceRuntime {
    fn new_external(config: &KeyboardTraceConfig) -> Self {
        Self {
            source_kind: "external_command",
            program: Some(config.program.clone()),
            args: config.args.clone(),
            socket_path: None,
            queue_capacity: config.channel_capacity,
            max_batch_size: config.max_batch_size,
            flush_interval: config.flush_interval,
            restart_backoff: config.restart_backoff,
            filter: config.filter.clone(),
            queue_depth: AtomicUsize::new(0),
            total_lines_seen: AtomicU64::new(0),
            total_non_event_lines: AtomicU64::new(0),
            total_parse_errors: AtomicU64::new(0),
            total_filtered_events: AtomicU64::new(0),
            total_enqueued_events: AtomicU64::new(0),
            total_published_events: AtomicU64::new(0),
            total_batches_flushed: AtomicU64::new(0),
            total_dropped_events: AtomicU64::new(0),
            restart_count: AtomicU64::new(0),
            last_started_at_ms: AtomicU64::new(0),
            last_event_at_ms: AtomicU64::new(0),
            last_exit_at_ms: AtomicU64::new(0),
            shared: RwLock::new(KeyboardTraceRuntimeShared::default()),
        }
    }

    fn new_socket(config: &KeyboardTraceSocketConfig) -> Self {
        Self {
            source_kind: "unix_socket",
            program: None,
            args: Vec::new(),
            socket_path: Some(config.socket_path.display().to_string()),
            queue_capacity: config.channel_capacity,
            max_batch_size: config.max_batch_size,
            flush_interval: config.flush_interval,
            restart_backoff: config.restart_backoff,
            filter: config.filter.clone(),
            queue_depth: AtomicUsize::new(0),
            total_lines_seen: AtomicU64::new(0),
            total_non_event_lines: AtomicU64::new(0),
            total_parse_errors: AtomicU64::new(0),
            total_filtered_events: AtomicU64::new(0),
            total_enqueued_events: AtomicU64::new(0),
            total_published_events: AtomicU64::new(0),
            total_batches_flushed: AtomicU64::new(0),
            total_dropped_events: AtomicU64::new(0),
            restart_count: AtomicU64::new(0),
            last_started_at_ms: AtomicU64::new(0),
            last_event_at_ms: AtomicU64::new(0),
            last_exit_at_ms: AtomicU64::new(0),
            shared: RwLock::new(KeyboardTraceRuntimeShared::default()),
        }
    }

    async fn snapshot(&self) -> KeyboardTraceStatusSnapshot {
        let shared = self.shared.read().await;

        KeyboardTraceStatusSnapshot {
            enabled: true,
            source_kind: self.source_kind,
            program: self.program.clone(),
            args: self.args.clone(),
            socket_path: self.socket_path.clone(),
            reader_state: shared.reader_state,
            queue_capacity: self.queue_capacity,
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            max_batch_size: self.max_batch_size,
            flush_interval_ms: self.flush_interval.as_millis() as u64,
            restart_backoff_ms: self.restart_backoff.as_millis() as u64,
            restart_count: self.restart_count.load(Ordering::Relaxed),
            total_lines_seen: self.total_lines_seen.load(Ordering::Relaxed),
            total_non_event_lines: self.total_non_event_lines.load(Ordering::Relaxed),
            total_parse_errors: self.total_parse_errors.load(Ordering::Relaxed),
            total_filtered_events: self.total_filtered_events.load(Ordering::Relaxed),
            total_enqueued_events: self.total_enqueued_events.load(Ordering::Relaxed),
            total_published_events: self.total_published_events.load(Ordering::Relaxed),
            total_batches_flushed: self.total_batches_flushed.load(Ordering::Relaxed),
            total_dropped_events: self.total_dropped_events.load(Ordering::Relaxed),
            last_started_at_ms: load_optional_atomic(&self.last_started_at_ms),
            last_event_at_ms: load_optional_atomic(&self.last_event_at_ms),
            last_exit_at_ms: load_optional_atomic(&self.last_exit_at_ms),
            last_exit_status: shared.last_exit_status.clone(),
            last_error: shared.last_error.clone(),
            filter: self.filter.clone(),
        }
    }

    async fn mark_starting(&self) {
        self.last_started_at_ms.store(now_ms(), Ordering::Relaxed);

        let mut shared = self.shared.write().await;
        shared.reader_state = KeyboardTraceReaderState::Starting;
        shared.last_error = None;
    }

    async fn mark_running(&self) {
        let mut shared = self.shared.write().await;
        shared.reader_state = KeyboardTraceReaderState::Running;
        shared.last_error = None;
    }

    async fn mark_backoff(&self, exit_status: Option<String>, error: Option<String>) {
        self.last_exit_at_ms.store(now_ms(), Ordering::Relaxed);

        let mut shared = self.shared.write().await;
        shared.reader_state = KeyboardTraceReaderState::Backoff;
        shared.last_exit_status = exit_status;
        shared.last_error = error;
    }

    async fn mark_stopped(&self, exit_status: Option<String>, error: Option<String>) {
        self.last_exit_at_ms.store(now_ms(), Ordering::Relaxed);

        let mut shared = self.shared.write().await;
        shared.reader_state = KeyboardTraceReaderState::Stopped;
        shared.last_exit_status = exit_status;
        shared.last_error = error;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TraceInputEvent {
    device_ptr: Option<String>,
    raw_event: RawInputEvent,
}

pub fn spawn_keyboard_trace_service(
    state: Arc<RwLock<TelemetryStore>>,
    config: KeyboardTraceConfig,
) -> anyhow::Result<KeyboardTraceHandle> {
    let channel_capacity = config.channel_capacity.max(1);
    let max_batch_size = config.max_batch_size.max(1);
    let flush_interval = config.flush_interval.max(Duration::from_millis(1));
    let restart_backoff = config.restart_backoff.max(Duration::from_millis(1));

    let normalized_config = KeyboardTraceConfig {
        program: config.program,
        args: config.args,
        channel_capacity,
        max_batch_size,
        flush_interval,
        restart_backoff,
        filter: config.filter,
    };

    let (sender, receiver) = mpsc::channel(channel_capacity);
    let pending_dropped_events = Arc::new(AtomicU64::new(0));
    let runtime = Arc::new(KeyboardTraceRuntime::new_external(&normalized_config));
    let processing = KeyboardTraceProcessingConfig {
        max_batch_size,
        flush_interval,
        filter: normalized_config.filter.clone(),
    };

    tokio::spawn(run_command_trace_reader(
        normalized_config,
        sender,
        Arc::clone(&pending_dropped_events),
        Arc::clone(&runtime),
    ));
    tokio::spawn(run_trace_processor(
        state,
        receiver,
        pending_dropped_events,
        Arc::clone(&runtime),
        processing,
    ));

    Ok(KeyboardTraceHandle { runtime })
}

pub fn spawn_keyboard_trace_socket_service(
    state: Arc<RwLock<TelemetryStore>>,
    config: KeyboardTraceSocketConfig,
) -> anyhow::Result<KeyboardTraceHandle> {
    let channel_capacity = config.channel_capacity.max(1);
    let max_batch_size = config.max_batch_size.max(1);
    let flush_interval = config.flush_interval.max(Duration::from_millis(1));
    let restart_backoff = config.restart_backoff.max(Duration::from_millis(1));

    let normalized_config = KeyboardTraceSocketConfig {
        socket_path: config.socket_path,
        channel_capacity,
        max_batch_size,
        flush_interval,
        restart_backoff,
        filter: config.filter,
    };

    let (sender, receiver) = mpsc::channel(channel_capacity);
    let pending_dropped_events = Arc::new(AtomicU64::new(0));
    let runtime = Arc::new(KeyboardTraceRuntime::new_socket(&normalized_config));
    let processing = KeyboardTraceProcessingConfig {
        max_batch_size,
        flush_interval,
        filter: normalized_config.filter.clone(),
    };

    tokio::spawn(run_socket_trace_reader(
        normalized_config,
        sender,
        Arc::clone(&pending_dropped_events),
        Arc::clone(&runtime),
    ));
    tokio::spawn(run_trace_processor(
        state,
        receiver,
        pending_dropped_events,
        Arc::clone(&runtime),
        processing,
    ));

    Ok(KeyboardTraceHandle { runtime })
}

async fn run_command_trace_reader(
    config: KeyboardTraceConfig,
    sender: mpsc::Sender<TraceInputEvent>,
    pending_dropped_events: Arc<AtomicU64>,
    runtime: Arc<KeyboardTraceRuntime>,
) {
    loop {
        if sender.is_closed() {
            runtime.mark_stopped(None, None).await;
            break;
        }

        runtime.mark_starting().await;

        match run_command_trace_reader_inner(
            &config,
            &sender,
            &pending_dropped_events,
            Arc::clone(&runtime),
        )
        .await
        {
            Ok(status) => {
                info!(?status, "keyboard trace command exited");

                if sender.is_closed() {
                    runtime.mark_stopped(Some(status.to_string()), None).await;
                    break;
                }

                runtime.restart_count.fetch_add(1, Ordering::Relaxed);
                runtime.mark_backoff(Some(status.to_string()), None).await;
            }
            Err(error) => {
                warn!(?error, "keyboard trace reader exited");

                if sender.is_closed() {
                    runtime.mark_stopped(None, Some(error.to_string())).await;
                    break;
                }

                runtime.restart_count.fetch_add(1, Ordering::Relaxed);
                runtime.mark_backoff(None, Some(error.to_string())).await;
            }
        }

        time::sleep(config.restart_backoff).await;
    }
}

async fn run_command_trace_reader_inner(
    config: &KeyboardTraceConfig,
    sender: &mpsc::Sender<TraceInputEvent>,
    pending_dropped_events: &AtomicU64,
    runtime: Arc<KeyboardTraceRuntime>,
) -> anyhow::Result<std::process::ExitStatus> {
    let mut child = Command::new(&config.program)
        .args(&config.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn keyboard trace command {}", config.program))?;

    runtime.mark_running().await;

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "watchmedo::input_trace", line, "keyboard trace stderr");
            }
        });
    }

    let stdout = child
        .stdout
        .take()
        .context("keyboard trace command did not provide stdout")?;

    info!(program = %config.program, args = ?config.args, filter = ?config.filter, "keyboard trace service started");

    read_trace_lines(
        BufReader::new(stdout).lines(),
        sender,
        pending_dropped_events,
        runtime,
        &config.filter,
    )
    .await
    .context("failed while reading keyboard trace stdout")?;

    child.wait().await.context("failed to wait on keyboard trace command")
}

async fn run_socket_trace_reader(
    config: KeyboardTraceSocketConfig,
    sender: mpsc::Sender<TraceInputEvent>,
    pending_dropped_events: Arc<AtomicU64>,
    runtime: Arc<KeyboardTraceRuntime>,
) {
    let socket_path = config.socket_path.clone();
    let listener = match bind_trace_socket(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            warn!(?error, path = %socket_path.display(), "failed to bind keyboard trace socket listener");
            runtime.mark_stopped(None, Some(error.to_string())).await;
            return;
        }
    };

    info!(path = %socket_path.display(), filter = ?config.filter, "keyboard trace socket listener started");

    loop {
        if sender.is_closed() {
            runtime.mark_stopped(None, None).await;
            break;
        }

        runtime.mark_starting().await;

        match listener.accept().await {
            Ok((stream, _)) => {
                runtime.mark_running().await;

                let read_result = read_trace_lines(
                    BufReader::new(stream).lines(),
                    &sender,
                    &pending_dropped_events,
                    Arc::clone(&runtime),
                    &config.filter,
                )
                .await;

                match read_result {
                    Ok(()) => {
                        if sender.is_closed() {
                            runtime.mark_stopped(Some("socket closed".to_owned()), None).await;
                            break;
                        }

                        runtime.restart_count.fetch_add(1, Ordering::Relaxed);
                        runtime
                            .mark_backoff(Some("socket peer disconnected".to_owned()), None)
                            .await;
                    }
                    Err(error) => {
                        if sender.is_closed() {
                            runtime.mark_stopped(None, Some(error.to_string())).await;
                            break;
                        }

                        runtime.restart_count.fetch_add(1, Ordering::Relaxed);
                        runtime.mark_backoff(None, Some(error.to_string())).await;
                    }
                }

                time::sleep(config.restart_backoff).await;
            }
            Err(error) => {
                if sender.is_closed() {
                    runtime.mark_stopped(None, Some(error.to_string())).await;
                    break;
                }

                runtime.restart_count.fetch_add(1, Ordering::Relaxed);
                runtime.mark_backoff(None, Some(error.to_string())).await;
                time::sleep(config.restart_backoff).await;
            }
        }
    }
}

async fn read_trace_lines<R>(
    mut lines: tokio::io::Lines<BufReader<R>>,
    sender: &mpsc::Sender<TraceInputEvent>,
    pending_dropped_events: &AtomicU64,
    runtime: Arc<KeyboardTraceRuntime>,
    filter: &KeyboardTraceFilter,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Some(line) = lines.next_line().await? {
        runtime.total_lines_seen.fetch_add(1, Ordering::Relaxed);

        match parse_bpftrace_line(&line) {
            Ok(Some(event)) => {
                if !filter.matches_device(event.device_ptr.as_deref()) {
                    runtime.total_filtered_events.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                match sender.try_send(event) {
                    Ok(()) => {
                        runtime.queue_depth.fetch_add(1, Ordering::Relaxed);
                        runtime.total_enqueued_events.fetch_add(1, Ordering::Relaxed);
                        runtime.last_event_at_ms.store(now_ms(), Ordering::Relaxed);
                    }
                    Err(TrySendError::Full(_)) => {
                        pending_dropped_events.fetch_add(1, Ordering::Relaxed);
                        runtime.total_dropped_events.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Closed(_)) => {
                        anyhow::bail!("keyboard trace processor closed");
                    }
                }
            }
            Ok(None) => {
                runtime.total_non_event_lines.fetch_add(1, Ordering::Relaxed);
            }
            Err(reason) => {
                runtime.total_parse_errors.fetch_add(1, Ordering::Relaxed);
                debug!(reason, %line, "failed to parse keyboard trace line");
            }
        }
    }

    Ok(())
}

async fn run_trace_processor(
    state: Arc<RwLock<TelemetryStore>>,
    mut receiver: mpsc::Receiver<TraceInputEvent>,
    pending_dropped_events: Arc<AtomicU64>,
    runtime: Arc<KeyboardTraceRuntime>,
    config: KeyboardTraceProcessingConfig,
) {
    let mut decoder = KeyboardEventDecoder::default();
    let mut batch = Vec::with_capacity(config.max_batch_size);
    let mut ticker = time::interval(config.flush_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_event = receiver.recv() => {
                match maybe_event {
                    Some(event) => {
                        runtime.queue_depth.fetch_sub(1, Ordering::Relaxed);

                        if let Some(decoded) = decoder.decode_event(event.raw_event) {
                            if !config.filter.matches_key(&decoded) {
                                runtime.total_filtered_events.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }

                            batch.push(PendingKeyboardEvent {
                                captured_at_ms: now_ms(),
                                device_ptr: event.device_ptr,
                                key: decoded,
                            });
                        }

                        if batch.len() >= config.max_batch_size {
                            flush_batch(&state, &mut batch, &pending_dropped_events, &runtime).await;
                        }
                    }
                    None => {
                        flush_batch(&state, &mut batch, &pending_dropped_events, &runtime).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !batch.is_empty() || pending_dropped_events.load(Ordering::Relaxed) > 0 {
                    flush_batch(&state, &mut batch, &pending_dropped_events, &runtime).await;
                }
            }
        }
    }
}

async fn flush_batch(
    state: &Arc<RwLock<TelemetryStore>>,
    batch: &mut Vec<PendingKeyboardEvent>,
    pending_dropped_events: &AtomicU64,
    runtime: &KeyboardTraceRuntime,
) {
    let dropped = pending_dropped_events.swap(0, Ordering::Relaxed);

    if batch.is_empty() && dropped == 0 {
        return;
    }

    let events = std::mem::take(batch);
    let count = events.len();

    {
        let mut guard = state.write().await;
        guard.insert_keyboard_batch(events, now_ms(), dropped);
    }

    runtime.total_batches_flushed.fetch_add(1, Ordering::Relaxed);
    runtime.total_published_events.fetch_add(count as u64, Ordering::Relaxed);

    debug!(count, dropped, "flushed keyboard event batch");
}

fn bind_trace_socket(path: &Path) -> anyhow::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create keyboard trace socket directory {}", parent.display()))?;
    }

    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove stale keyboard trace socket {}", path.display()))?;
    }

    UnixListener::bind(path)
        .with_context(|| format!("failed to bind keyboard trace socket {}", path.display()))
}

fn parse_bpftrace_line(line: &str) -> Result<Option<TraceInputEvent>, &'static str> {
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

    Ok(Some(TraceInputEvent {
        device_ptr: Some(device_ptr),
        raw_event: RawInputEvent::new(event_type, code, value),
    }))
}

fn parse_field<'a>(payload: &'a str, key: &str) -> Result<&'a str, &'static str> {
    let token = format!("{key}=");
    let start = payload.find(&token).ok_or("missing field")? + token.len();
    let rest = &payload[start..];

    rest.split_whitespace().next().ok_or("missing field value")
}

fn load_optional_atomic(value: &AtomicU64) -> Option<u64> {
    let current = value.load(Ordering::Relaxed);

    if current == 0 {
        None
    } else {
        Some(current)
    }
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
    use super::{KeyboardTraceFilter, KeyboardTraceStateFilter, parse_bpftrace_line};
    use crate::input_keys::{KeyState, KeyboardKeyEvent, Modifiers};

    #[test]
    fn parses_plain_bpftrace_lines() {
        let parsed = parse_bpftrace_line("dev_ptr=0xffff894a62670000 type=1 code=20 value=1")
            .expect("expected valid parse result")
            .expect("expected parsed trace line");

        assert_eq!(parsed.device_ptr.as_deref(), Some("0xffff894a62670000"));
        assert_eq!(parsed.raw_event.event_type, 1);
        assert_eq!(parsed.raw_event.code, 20);
        assert_eq!(parsed.raw_event.value, 1);
    }

    #[test]
    fn ignores_tty_prefix_noise_before_dev_ptr_marker() {
        let parsed = parse_bpftrace_line("tdev_ptr=0xffff894a62670000 type=1 code=20 value=1")
            .expect("expected valid parse result")
            .expect("expected parsed trace line");

        assert_eq!(parsed.device_ptr.as_deref(), Some("0xffff894a62670000"));
        assert_eq!(parsed.raw_event.code, 20);
    }

    #[test]
    fn rejects_non_matching_lines() {
        assert!(parse_bpftrace_line("Attached 1 probe")
            .expect("expected ignored line")
            .is_none());
    }

    #[test]
    fn filters_device_ptrs_and_key_states() {
        let filter = KeyboardTraceFilter {
            device_ptrs: vec!["0xffff894a62670000".to_owned()],
            states: vec![KeyboardTraceStateFilter::Down],
        };

        assert!(filter.matches_device(Some("0xffff894a62670000")));
        assert!(!filter.matches_device(Some("0xffffdeadbeef")));
        assert!(filter.matches_key(&KeyboardKeyEvent {
            linux_code: 20,
            key_name: "KEY_T".to_owned(),
            state: KeyState::Down,
            raw_scancode: Some(458775),
            modifiers: Modifiers::default(),
        }));
        assert!(!filter.matches_key(&KeyboardKeyEvent {
            linux_code: 20,
            key_name: "KEY_T".to_owned(),
            state: KeyState::Up,
            raw_scancode: Some(458775),
            modifiers: Modifiers::default(),
        }));
    }
}