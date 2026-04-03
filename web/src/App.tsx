import { type CSSProperties, startTransition, useDeferredValue, useEffect, useState } from "react";

type NodeMetadata = {
    host_name: string | null;
    system_name: string | null;
    os_version: string | null;
    kernel_version: string | null;
    cpu_count: number;
    physical_core_count: number | null;
};

type SystemSample = {
    sequence: number;
    captured_at_ms: number;
    cpu_usage_pct: number;
    load_average_one: number;
    load_average_five: number;
    load_average_fifteen: number;
    total_memory_bytes: number;
    used_memory_bytes: number;
    available_memory_bytes: number;
    total_swap_bytes: number;
    used_swap_bytes: number;
    process_count: number;
    total_disk_bytes: number;
    available_disk_bytes: number;
    network_received_bytes: number;
    network_transmitted_bytes: number;
    total_network_received_bytes: number;
    total_network_transmitted_bytes: number;
};

type ProcessSummary = {
    pid: string;
    name: string;
    cpu_usage_pct: number;
    memory_bytes: number;
    virtual_memory_bytes: number;
    status: string;
};

type HistoryResponse = {
    generated_at_ms: number;
    retention_secs: number;
    sample_interval_ms: number;
    node: NodeMetadata;
    samples: SystemSample[];
    top_processes: ProcessSummary[];
};

type LiveTelemetryEvent = {
    published_at_ms: number;
    sample: SystemSample;
    top_processes: ProcessSummary[];
};

type Modifiers = {
    ctrl: boolean;
    shift: boolean;
    alt: boolean;
    meta: boolean;
};

type KeyboardState = "up" | "down" | "repeat";

type KeyboardInputEvent = {
    sequence: number;
    captured_at_ms: number;
    device_ptr: string | null;
    linux_code: number;
    key_name: string;
    state: KeyboardState;
    raw_scancode: number | null;
    modifiers: Modifiers;
};

type KeyboardEventBatch = {
    published_at_ms: number;
    dropped_events: number;
    events: KeyboardInputEvent[];
};

type KeyboardHistoryResponse = {
    generated_at_ms: number;
    retention_secs: number;
    dropped_events: number;
    events: KeyboardInputEvent[];
};

type KeyboardTraceFilter = {
    device_ptrs: string[];
    states: string[];
};

type KeyboardTraceStatusSnapshot = {
    enabled: boolean;
    source_kind: string;
    program: string | null;
    args: string[];
    socket_path: string | null;
    reader_state: string;
    queue_capacity: number;
    queue_depth: number;
    max_batch_size: number;
    flush_interval_ms: number;
    restart_backoff_ms: number;
    restart_count: number;
    total_lines_seen: number;
    total_non_event_lines: number;
    total_parse_errors: number;
    total_filtered_events: number;
    total_enqueued_events: number;
    total_published_events: number;
    total_batches_flushed: number;
    total_dropped_events: number;
    last_started_at_ms: number | null;
    last_event_at_ms: number | null;
    last_exit_at_ms: number | null;
    last_exit_status: string | null;
    last_error: string | null;
    filter: KeyboardTraceFilter;
};

type KeyboardStoreStatusSnapshot = {
    retained_events: number;
    dropped_events: number;
    max_events: number;
    next_sequence: number;
    oldest_event_at_ms: number | null;
    newest_event_at_ms: number | null;
    live_subscribers: number;
};

type KeyboardStatusResponse = {
    timestamp_ms: number;
    trace: KeyboardTraceStatusSnapshot;
    store: KeyboardStoreStatusSnapshot;
};

type ShellCommandEvent = {
    sequence: number;
    captured_at_ms: number;
    pid: number;
    uid: number;
    command: string;
    executable: string | null;
};

type ShellCommandBatch = {
    published_at_ms: number;
    events: ShellCommandEvent[];
};

type ShellHistoryResponse = {
    generated_at_ms: number;
    retention_secs: number;
    events: ShellCommandEvent[];
};

type ShellStoreStatusSnapshot = {
    retained_events: number;
    max_events: number;
    next_sequence: number;
    oldest_event_at_ms: number | null;
    newest_event_at_ms: number | null;
    live_subscribers: number;
};

type ShellStatusResponse = {
    timestamp_ms: number;
    store: ShellStoreStatusSnapshot;
};

type KeyDefinition = {
    code: string;
    label: string;
    secondary?: string;
    width?: number;
    tone?: "neutral" | "warm" | "cool";
};

type TranscriptTone = "text" | "command" | "edit" | "navigation";

type TranscriptToken = {
    id: string;
    text: string;
    tone: TranscriptTone;
};

type RenderedAction = {
    sequence: number;
    label: string;
    tone: TranscriptTone;
    capturedAtMs: number;
};

type KeyVisualState = {
    pressed: boolean;
    pulsing: boolean;
};

type ProbeDescriptor = {
    kind: string;
    name: string;
};

type KeyboardScene = {
    transcript: string;
    lines: TranscriptToken[][];
    actions: RenderedAction[];
    keyStates: Map<string, KeyVisualState>;
    activeCount: number;
};

const API_BASE = (import.meta.env.VITE_WATCHMEDO_API_BASE as string | undefined)?.replace(/\/$/, "") ?? "";
const HISTORY_LOOKBACK_SECS = 30;
const KEYBOARD_LOOKBACK_SECS = 180;
const SHELL_LOOKBACK_SECS = 1_800;
const MAX_SAMPLES = 90;
const MAX_KEYBOARD_EVENTS = 720;
const MAX_SHELL_EVENTS = 320;
const KEYBOARD_PULSE_MS = 180;
const MAX_TRANSCRIPT_CHARS = 2800;
const MAX_TRANSCRIPT_TOKENS = 180;

const NAVIGATION_CHORD_KEYS = new Set([
    "KEY_LEFT",
    "KEY_RIGHT",
    "KEY_UP",
    "KEY_DOWN",
    "KEY_HOME",
    "KEY_END",
    "KEY_PAGEUP",
    "KEY_PAGEDOWN",
    "KEY_TAB",
]);

const SUPER_NAVIGATION_KEYS = new Set(["KEY_H", "KEY_J", "KEY_K", "KEY_L"]);

const MAIN_KEYBOARD_ROWS: KeyDefinition[][] = [
    [
        { code: "KEY_ESC", label: "Esc", width: 1.15 },
        { code: "KEY_1", label: "1", secondary: "!" },
        { code: "KEY_2", label: "2", secondary: "@" },
        { code: "KEY_3", label: "3", secondary: "#" },
        { code: "KEY_4", label: "4", secondary: "$" },
        { code: "KEY_5", label: "5", secondary: "%" },
        { code: "KEY_6", label: "6", secondary: "^" },
        { code: "KEY_7", label: "7", secondary: "&" },
        { code: "KEY_8", label: "8", secondary: "*" },
        { code: "KEY_9", label: "9", secondary: "(" },
        { code: "KEY_0", label: "0", secondary: ")" },
        { code: "KEY_MINUS", label: "-", secondary: "_" },
        { code: "KEY_EQUAL", label: "=", secondary: "+" },
        { code: "KEY_BACKSPACE", label: "Delete", width: 2.2, tone: "warm" },
    ],
    [
        { code: "KEY_TAB", label: "Tab", width: 1.55 },
        { code: "KEY_Q", label: "Q" },
        { code: "KEY_W", label: "W" },
        { code: "KEY_E", label: "E" },
        { code: "KEY_R", label: "R" },
        { code: "KEY_T", label: "T" },
        { code: "KEY_Y", label: "Y" },
        { code: "KEY_U", label: "U" },
        { code: "KEY_I", label: "I" },
        { code: "KEY_O", label: "O" },
        { code: "KEY_P", label: "P" },
        { code: "KEY_LEFTBRACE", label: "[", secondary: "{" },
        { code: "KEY_RIGHTBRACE", label: "]", secondary: "}" },
        { code: "KEY_BACKSLASH", label: "\\", secondary: "|", width: 1.65 },
    ],
    [
        { code: "KEY_CAPSLOCK", label: "Caps", width: 1.9 },
        { code: "KEY_A", label: "A" },
        { code: "KEY_S", label: "S" },
        { code: "KEY_D", label: "D" },
        { code: "KEY_F", label: "F" },
        { code: "KEY_G", label: "G" },
        { code: "KEY_H", label: "H" },
        { code: "KEY_J", label: "J" },
        { code: "KEY_K", label: "K" },
        { code: "KEY_L", label: "L" },
        { code: "KEY_SEMICOLON", label: ";", secondary: ":" },
        { code: "KEY_APOSTROPHE", label: "'", secondary: '"' },
        { code: "KEY_ENTER", label: "Enter", width: 2.35, tone: "cool" },
    ],
    [
        { code: "KEY_LEFTSHIFT", label: "Shift", width: 2.35, tone: "cool" },
        { code: "KEY_Z", label: "Z" },
        { code: "KEY_X", label: "X" },
        { code: "KEY_C", label: "C" },
        { code: "KEY_V", label: "V" },
        { code: "KEY_B", label: "B" },
        { code: "KEY_N", label: "N" },
        { code: "KEY_M", label: "M" },
        { code: "KEY_COMMA", label: ",", secondary: "<" },
        { code: "KEY_DOT", label: ".", secondary: ">" },
        { code: "KEY_SLASH", label: "/", secondary: "?" },
        { code: "KEY_RIGHTSHIFT", label: "Shift", width: 2.85, tone: "cool" },
    ],
    [
        { code: "KEY_LEFTCTRL", label: "Ctrl", width: 1.45, tone: "cool" },
        { code: "KEY_LEFTMETA", label: "Super", width: 1.35 },
        { code: "KEY_LEFTALT", label: "Alt", width: 1.35 },
        { code: "KEY_SPACE", label: "Space", width: 6.25 },
        { code: "KEY_RIGHTALT", label: "Alt", width: 1.35 },
        { code: "KEY_RIGHTMETA", label: "Super", width: 1.35 },
        { code: "KEY_MENU", label: "Menu", width: 1.2 },
        { code: "KEY_RIGHTCTRL", label: "Ctrl", width: 1.45, tone: "cool" },
    ],
];

const NAVIGATION_CLUSTER: KeyDefinition[][] = [
    [{ code: "KEY_UP", label: "↑", width: 1.1 }],
    [
        { code: "KEY_LEFT", label: "←", width: 1.1 },
        { code: "KEY_DOWN", label: "↓", width: 1.1 },
        { code: "KEY_RIGHT", label: "→", width: 1.1 },
    ],
];

function App() {
    const [history, setHistory] = useState<HistoryResponse | null>(null);
    const [samples, setSamples] = useState<SystemSample[]>([]);
    const [topProcesses, setTopProcesses] = useState<ProcessSummary[]>([]);
    const [telemetryConnectionState, setTelemetryConnectionState] = useState("booting");
    const [telemetryErrorMessage, setTelemetryErrorMessage] = useState<string | null>(null);
    const [lastTelemetryAtMs, setLastTelemetryAtMs] = useState<number | null>(null);

    const [keyboardEvents, setKeyboardEvents] = useState<KeyboardInputEvent[]>([]);
    const [keyboardStatus, setKeyboardStatus] = useState<KeyboardStatusResponse | null>(null);
    const [keyboardConnectionState, setKeyboardConnectionState] = useState("booting");
    const [keyboardErrorMessage, setKeyboardErrorMessage] = useState<string | null>(null);
    const [lastKeyboardAtMs, setLastKeyboardAtMs] = useState<number | null>(null);
    const [shellCommands, setShellCommands] = useState<ShellCommandEvent[]>([]);
    const [shellStatus, setShellStatus] = useState<ShellStatusResponse | null>(null);
    const [shellConnectionState, setShellConnectionState] = useState("booting");
    const [shellErrorMessage, setShellErrorMessage] = useState<string | null>(null);
    const [lastShellAtMs, setLastShellAtMs] = useState<number | null>(null);
    const [clockMs, setClockMs] = useState(() => Date.now());

    const deferredSamples = useDeferredValue(samples);
    const deferredKeyboardEvents = useDeferredValue(keyboardEvents);
    const deferredShellCommands = useDeferredValue(shellCommands);
    const currentSample = deferredSamples[deferredSamples.length - 1] ?? null;
    const keyboardScene = buildKeyboardScene(deferredKeyboardEvents, clockMs);

    useEffect(() => {
        const timer = window.setInterval(() => {
            setClockMs(Date.now());
        }, 120);

        return () => window.clearInterval(timer);
    }, []);

    useEffect(() => {
        const controller = new AbortController();

        async function loadHistory() {
            try {
                const params = new URLSearchParams({
                    lookbackSecs: String(HISTORY_LOOKBACK_SECS),
                    includeProcesses: "true",
                    maxProcesses: "8",
                });

                const payload = await fetchJson<HistoryResponse>(`${API_BASE}/api/history?${params.toString()}`, controller.signal);

                startTransition(() => {
                    setHistory(payload);
                    setSamples(payload.samples);
                    setTopProcesses(payload.top_processes);
                    setTelemetryConnectionState("history ready");
                    setTelemetryErrorMessage(null);
                });
            } catch (error) {
                if (controller.signal.aborted) {
                    return;
                }

                setTelemetryConnectionState("history failed");
                setTelemetryErrorMessage(error instanceof Error ? error.message : "failed to load history");
            }
        }

        void loadHistory();

        return () => controller.abort();
    }, []);

    useEffect(() => {
        const controller = new AbortController();

        async function loadShellBootstrap() {
            try {
                const params = new URLSearchParams({
                    lookbackSecs: String(SHELL_LOOKBACK_SECS),
                    maxEvents: String(MAX_SHELL_EVENTS),
                });

                const [historyPayload, statusPayload] = await Promise.all([
                    fetchJson<ShellHistoryResponse>(`${API_BASE}/api/shell/history?${params.toString()}`, controller.signal),
                    fetchJson<ShellStatusResponse>(`${API_BASE}/api/shell/status`, controller.signal),
                ]);

                const latestShellEvent = historyPayload.events[historyPayload.events.length - 1];

                startTransition(() => {
                    setShellCommands(historyPayload.events);
                    setShellStatus(statusPayload);
                    setLastShellAtMs(latestShellEvent?.captured_at_ms ?? statusPayload.store.newest_event_at_ms);
                    setShellConnectionState("armed");
                    setShellErrorMessage(null);
                });
            } catch (error) {
                if (controller.signal.aborted) {
                    return;
                }

                setShellConnectionState("history failed");
                setShellErrorMessage(error instanceof Error ? error.message : "failed to load shell history");
            }
        }

        void loadShellBootstrap();

        return () => controller.abort();
    }, []);

    useEffect(() => {
        const controller = new AbortController();

        async function loadKeyboardBootstrap() {
            try {
                const params = new URLSearchParams({
                    lookbackSecs: String(KEYBOARD_LOOKBACK_SECS),
                    maxEvents: String(MAX_KEYBOARD_EVENTS),
                });

                const [historyPayload, statusPayload] = await Promise.all([
                    fetchJson<KeyboardHistoryResponse>(`${API_BASE}/api/keyboard/history?${params.toString()}`, controller.signal),
                    fetchJson<KeyboardStatusResponse>(`${API_BASE}/api/keyboard/status`, controller.signal),
                ]);

                const latestKeyboardEvent = historyPayload.events[historyPayload.events.length - 1];

                startTransition(() => {
                    setKeyboardEvents(historyPayload.events);
                    setKeyboardStatus(statusPayload);
                    setLastKeyboardAtMs(latestKeyboardEvent?.captured_at_ms ?? statusPayload.trace.last_event_at_ms);
                    setKeyboardConnectionState(statusPayload.trace.enabled ? "armed" : "disabled");
                    setKeyboardErrorMessage(null);
                });
            } catch (error) {
                if (controller.signal.aborted) {
                    return;
                }

                setKeyboardConnectionState("history failed");
                setKeyboardErrorMessage(error instanceof Error ? error.message : "failed to load keyboard history");
            }
        }

        void loadKeyboardBootstrap();

        return () => controller.abort();
    }, []);

    useEffect(() => {
        let cancelled = false;

        async function loadKeyboardStatus() {
            try {
                const payload = await fetchJson<KeyboardStatusResponse>(`${API_BASE}/api/keyboard/status`);

                if (cancelled) {
                    return;
                }

                startTransition(() => {
                    setKeyboardStatus(payload);

                    if (!payload.trace.enabled) {
                        setKeyboardConnectionState((current) => (current === "live" ? current : "disabled"));
                    }
                });
            } catch (error) {
                if (cancelled) {
                    return;
                }

                setKeyboardErrorMessage(error instanceof Error ? error.message : "failed to refresh keyboard status");
            }
        }

        void loadKeyboardStatus();

        const timer = window.setInterval(() => {
            void loadKeyboardStatus();
        }, 4000);

        return () => {
            cancelled = true;
            window.clearInterval(timer);
        };
    }, []);

    useEffect(() => {
        let cancelled = false;

        async function loadShellStatus() {
            try {
                const payload = await fetchJson<ShellStatusResponse>(`${API_BASE}/api/shell/status`);

                if (cancelled) {
                    return;
                }

                startTransition(() => {
                    setShellStatus(payload);
                });
            } catch (error) {
                if (cancelled) {
                    return;
                }

                setShellErrorMessage(error instanceof Error ? error.message : "failed to refresh shell status");
            }
        }

        void loadShellStatus();

        const timer = window.setInterval(() => {
            void loadShellStatus();
        }, 4000);

        return () => {
            cancelled = true;
            window.clearInterval(timer);
        };
    }, []);

    useEffect(() => {
        const stream = new EventSource(`${API_BASE}/api/live`);

        stream.addEventListener("open", () => {
            setTelemetryConnectionState((state) => (state === "history ready" ? "live" : state));
            setTelemetryErrorMessage(null);
        });

        stream.addEventListener("telemetry", (event) => {
            const message = event as MessageEvent<string>;

            try {
                const payload = JSON.parse(message.data) as LiveTelemetryEvent;

                startTransition(() => {
                    setSamples((previous) => [...previous, payload.sample].slice(-MAX_SAMPLES));
                    setTopProcesses(payload.top_processes);
                    setLastTelemetryAtMs(payload.published_at_ms);
                    setTelemetryConnectionState("live");
                });
            } catch (error) {
                setTelemetryErrorMessage(error instanceof Error ? error.message : "failed to decode live update");
            }
        });

        stream.addEventListener("error", () => {
            setTelemetryConnectionState("reconnecting");
        });

        return () => {
            stream.close();
        };
    }, []);

    useEffect(() => {
        const stream = new EventSource(`${API_BASE}/api/keyboard/live`);

        stream.addEventListener("open", () => {
            setKeyboardConnectionState((state) => {
                if (state === "disabled") {
                    return state;
                }

                return "listening";
            });
            setKeyboardErrorMessage(null);
        });

        stream.addEventListener("keyboard", (event) => {
            const message = event as MessageEvent<string>;

            try {
                const payload = JSON.parse(message.data) as KeyboardEventBatch;

                startTransition(() => {
                    setKeyboardEvents((previous) => [...previous, ...payload.events].slice(-MAX_KEYBOARD_EVENTS));
                    setLastKeyboardAtMs(payload.published_at_ms);
                    setKeyboardConnectionState("live");
                });
            } catch (error) {
                setKeyboardErrorMessage(error instanceof Error ? error.message : "failed to decode keyboard update");
            }
        });

        stream.addEventListener("error", () => {
            setKeyboardConnectionState((state) => (state === "disabled" ? state : "reconnecting"));
        });

        return () => {
            stream.close();
        };
    }, []);

    useEffect(() => {
        const stream = new EventSource(`${API_BASE}/api/shell/live`);

        stream.addEventListener("open", () => {
            setShellConnectionState("listening");
            setShellErrorMessage(null);
        });

        stream.addEventListener("shell", (event) => {
            const message = event as MessageEvent<string>;

            try {
                const payload = JSON.parse(message.data) as ShellCommandBatch;

                startTransition(() => {
                    setShellCommands((previous) => [...previous, ...payload.events].slice(-MAX_SHELL_EVENTS));
                    setLastShellAtMs(payload.published_at_ms);
                    setShellConnectionState("live");
                });
            } catch (error) {
                setShellErrorMessage(error instanceof Error ? error.message : "failed to decode shell update");
            }
        });

        stream.addEventListener("error", () => {
            setShellConnectionState("reconnecting");
        });

        return () => {
            stream.close();
        };
    }, []);

    const cpuSeries = deferredSamples.map((sample) => sample.cpu_usage_pct);
    const memorySeries = deferredSamples.map((sample) => percentage(sample.used_memory_bytes, sample.total_memory_bytes));
    const networkSeries = deferredSamples.map((sample) => sample.network_received_bytes + sample.network_transmitted_bytes);
    const usedMemoryPct = currentSample ? percentage(currentSample.used_memory_bytes, currentSample.total_memory_bytes) : 0;
    const diskUsedPct = currentSample
        ? percentage(currentSample.total_disk_bytes - currentSample.available_disk_bytes, currentSample.total_disk_bytes)
        : 0;
    const telemetryFreshness = lastTelemetryAtMs ? formatFreshness(lastTelemetryAtMs, clockMs) : "pending";
    const keyboardFreshness = lastKeyboardAtMs ? formatFreshness(lastKeyboardAtMs, clockMs) : "pending";
    const shellFreshness = lastShellAtMs ? formatFreshness(lastShellAtMs, clockMs) : "pending";
    const transcriptWordCount = countWords(keyboardScene.transcript);
    const printableCount = keyboardScene.transcript.replace(/\s/g, "").length;
    const traceSource = keyboardStatus ? formatKeyboardSource(keyboardStatus.trace) : "probing";
    const traceHint = keyboardStatus?.trace.socket_path ?? keyboardStatus?.trace.program ?? "idle";
    const traceHealth = keyboardStatus?.trace.reader_state ?? "stopped";
    const droppedKeyboardEvents = keyboardStatus?.store.dropped_events ?? 0;
    const filteredKeyboardEvents = keyboardStatus?.trace.total_filtered_events ?? 0;
    const queueDepth = keyboardStatus?.trace.queue_depth ?? 0;
    const traceEnabled = keyboardStatus?.trace.enabled ?? false;
    const keyboardStatusTone = traceEnabled ? readerStateAccent(traceHealth) : "neutral";
    const compactActions = keyboardScene.actions.slice(0, 8);
    const shellCommandsPerMinute = countRecentShellCommands(deferredShellCommands, clockMs, 60_000);
    const topExecutable = mostExecutedBinary(deferredShellCommands);
    const shellUniqueExecutables = uniqueExecutables(deferredShellCommands);
    const latestShellCommands = deferredShellCommands.slice(-14).reverse();
    const latestShellSequence = deferredShellCommands.length > 0 ? deferredShellCommands[deferredShellCommands.length - 1].sequence : 0;
    const keyboardProbe = formatKeyboardProbe(keyboardStatus?.trace);
    const shellProbe = formatShellProbe();
    const keyboardProbeDetail = `${keyboardConnectionState} | ${keyboardFreshness}`;
    const shellProbeDetail = `${shellConnectionState} | ${shellFreshness}`;

    return (
        <main className="shell">
            <section className="hero panel">
                <div className="hero-copy">
                    <p className="eyebrow">watchmedo / Home</p>
                    <h1>Lets go!!!!!</h1>
                </div>
                <div className="status-stack">
                    <StatusPill
                        label="telemetry"
                        value={telemetryConnectionState}
                        detail={telemetryFreshness}
                        accent={connectionAccent(telemetryConnectionState)}
                    />
                    <StatusPill
                        label={keyboardProbe.kind}
                        value={keyboardProbe.name}
                        detail={keyboardProbeDetail}
                        accent={connectionAccent(keyboardConnectionState)}
                    />
                    <StatusPill
                        label={shellProbe.kind}
                        value={shellProbe.name}
                        detail={shellProbeDetail}
                        accent={connectionAccent(shellConnectionState)}
                    />
                </div>
            </section>

            <section className="keyboard-grid">
                <article className="panel keyboard-stage">
                    <div className="panel-head keyboard-head">
                        <div>
                            <h3>Inputs</h3>
                            {/* <p className="eyebrow">virtual keyboard</p> */}
                            {/* <h2>Live hardware impression</h2> */}
                        </div>
                        <div className="trace-ribbon">
                            <TraceBadge label="reader" value={traceHealth} tone={keyboardStatusTone} />
                            <TraceBadge label="queue" value={`${queueDepth}/${keyboardStatus?.trace.queue_capacity ?? 0}`} tone="neutral" />
                            <TraceBadge label="drops" value={String(droppedKeyboardEvents)} tone={droppedKeyboardEvents > 0 ? "warm" : "cool"} />
                        </div>
                    </div>

                    <div className="keyboard-summary-grid">
                        <MetricCard label="active keys" value={String(keyboardScene.activeCount)} detail="currently held down or repeating" compact />
                        <MetricCard label="printed glyphs" value={String(printableCount)} detail="approximate reconstructed characters" compact />
                        <MetricCard label="filtered" value={String(filteredKeyboardEvents)} detail="events trimmed by device or state filters" compact />
                        <MetricCard label="retained" value={String(keyboardStatus?.store.retained_events ?? keyboardEvents.length)} detail="events preserved in local ring history" compact />
                    </div>

                    <div className="desktop-keyboard-shell">
                        <div className="keyboard-deck-shell">
                            <div className="keyboard-deck-main">
                                {MAIN_KEYBOARD_ROWS.map((row, index) => (
                                    <div className="keyboard-row" key={`main-${index}`}>
                                        {row.map((keyDef) => (
                                            <KeyboardKeyCap key={keyDef.code} keyDef={keyDef} visualState={keyboardScene.keyStates.get(keyDef.code)} />
                                        ))}
                                    </div>
                                ))}
                            </div>
                            <div className="keyboard-deck-nav">
                                {NAVIGATION_CLUSTER.map((row, index) => (
                                    <div className="keyboard-row compact" key={`nav-${index}`}>
                                        {row.map((keyDef) => (
                                            <KeyboardKeyCap key={keyDef.code} keyDef={keyDef} visualState={keyboardScene.keyStates.get(keyDef.code)} />
                                        ))}
                                    </div>
                                ))}
                            </div>
                        </div>

                        <div className="trace-footer">
                            <div>
                                <p className="trace-label">trace endpoint</p>
                                <p className="trace-value">{traceHint}</p>
                            </div>
                            <div>
                                <p className="trace-label">device filters</p>
                                <p className="trace-value">{keyboardStatus?.trace.filter.device_ptrs.length ? keyboardStatus.trace.filter.device_ptrs.join(", ") : "all devices"}</p>
                            </div>
                            <div>
                                <p className="trace-label">state filters</p>
                                <p className="trace-value">{keyboardStatus?.trace.filter.states.length ? keyboardStatus.trace.filter.states.join(", ") : "down, repeat, up"}</p>
                            </div>
                        </div>
                    </div>

                    <div className="mobile-keyboard-shell">
                        <div className="subsection-head compact-mobile-head">
                            <p className="eyebrow">Most recent:</p>
                        {/* <span className="microcopy">most recent: </span> */}
                        </div>
                        <div className="mobile-key-grid">
                            {compactActions.length === 0 ? (
                                <p className="empty-state mobile-empty-state">No keyboard actions have been decoded yet.</p>
                            ) : (
                                compactActions.map((action) => <MobileKeyPulseCard key={action.sequence} action={action} nowMs={clockMs} />)
                            )}
                        </div>
                        <div className="mobile-trace-summary">
                            <TraceBadge label="source" value={traceSource} tone={keyboardStatusTone} />
                            <TraceBadge label="last key" value={keyboardFreshness} tone="warm" />
                            <TraceBadge label="drops" value={String(droppedKeyboardEvents)} tone={droppedKeyboardEvents > 0 ? "warm" : "cool"} />
                        </div>
                    </div>
                </article>

                <article className="panel transcript-panel">
                    <div className="panel-head transcript-head">
                        <div>
                            <p className="eyebrow">Transcript</p>
                            {/* <h3>Transcript</h3> */}
                            {/* <p className="eyebrow">reconstructed buffer</p> */}
                            {/* <h2>Words assembled after the fact</h2> */}
                        </div>
                        <div className="trace-ribbon">
                            <TraceBadge label="words" value={String(transcriptWordCount)} tone="cool" />
                            <TraceBadge label="events" value={String(deferredKeyboardEvents.length)} tone="neutral" />
                            <TraceBadge label="subscribers" value={String(keyboardStatus?.store.live_subscribers ?? 0)} tone="neutral" />
                        </div>
                    </div>

                    <div className="transcript-screen" aria-live="polite">
                        {keyboardScene.lines.length === 0 ? (
                            <p className="transcript-line ghost">
                                    <span className="transcript-token transcript-token-text ghost">Type, press chords, or feed history into the node to watch the transcript rebuild itself.</span>
                            </p>
                        ) : (
                            keyboardScene.lines.map((line, index) => (
                                <p className={`transcript-line ${line.length === 0 ? "ghost" : ""}`} key={`line-${index}`}>
                                    {line.length === 0 ? (
                                        <span className="transcript-token transcript-token-text ghost">\u00A0</span>
                                    ) : (
                                        line.map((token) => (
                                            <span className={`transcript-token transcript-token-${token.tone}`} key={token.id}>
                                                {token.text}
                                            </span>
                                        ))
                                    )}
                                </p>
                            ))
                        )}
                    </div>

                    {!traceEnabled ? (
                        <div className="callout">
                            <p className="callout-title">Keyboard tracing is currently disabled.</p>
                            <p className="callout-copy">Run the web and p2p node unprivileged, then attach the privileged input sidecar over a private Unix socket.</p>
                            <div className="callout-code">watchmedo serve --input-trace-socket /tmp/watchmedo-input.sock</div>
                            <div className="callout-code">sudo watchmedo-input-sidecar --socket-path /tmp/watchmedo-input.sock</div>
                        </div>
                    ) : null}

                    <div className="panel-subsection desktop-action-subsection">
                        <div className="subsection-head">
                            <p className="eyebrow">recent tape</p>
                            <span className="microcopy">latest keydown and repeat actions</span>
                        </div>
                        <div className="action-feed">
                            {keyboardScene.actions.length === 0 ? (
                                <p className="empty-state">No keyboard actions have been decoded yet.</p>
                            ) : (
                                keyboardScene.actions.map((action) => (
                                    <div className={`action-row ${action.tone}`} key={action.sequence}>
                                        <span className="action-sequence">#{action.sequence}</span>
                                        <strong>{action.label}</strong>
                                        <span>{formatFreshness(action.capturedAtMs, clockMs)}</span>
                                    </div>
                                ))
                            )}
                        </div>
                    </div>

                    {keyboardErrorMessage ? <p className="error-banner">{keyboardErrorMessage}</p> : null}
                </article>
            </section>

            <section className="shell-command-grid">
                <article className="panel shell-console-panel">
                    <div className="panel-head">
                        <div>
                            <p className="eyebrow">shell tape</p>
                            <h2>Live command history</h2>
                        </div>
                        <div className="trace-ribbon">
                            <TraceBadge label="cpm" value={String(shellCommandsPerMinute)} tone={shellCommandsPerMinute > 0 ? "cool" : "neutral"} />
                            <TraceBadge label="top binary" value={topExecutable} tone="neutral" />
                            <TraceBadge label="last command" value={shellFreshness} tone="warm" />
                        </div>
                    </div>

                    <div className="shell-summary-grid">
                        <MetricCard label="retained" value={String(shellStatus?.store.retained_events ?? deferredShellCommands.length)} detail="commands held in local shell history" compact />
                        <MetricCard label="unique bins" value={String(shellUniqueExecutables)} detail="distinct executables seen in the current window" compact />
                        <MetricCard label="subscribers" value={String(shellStatus?.store.live_subscribers ?? 0)} detail="live shell SSE consumers" compact />
                        <MetricCard label="next seq" value={String(shellStatus?.store.next_sequence ?? latestShellSequence + 1)} detail="sequence to be assigned to the next shell event" compact />
                    </div>

                    <div className="terminal-window" aria-live="polite">
                        <div className="terminal-chrome">
                            <span className="terminal-dot warm" />
                            <span className="terminal-dot neutral" />
                            <span className="terminal-dot cool" />
                            <span className="terminal-title">zsh global history</span>
                        </div>

                        <div className="terminal-buffer">
                            {latestShellCommands.length === 0 ? (
                                <p className="empty-state terminal-empty">No shell commands captured yet. Enable the shell sidecar module to begin streaming zsh command history.</p>
                            ) : (
                                latestShellCommands.map((event) => (
                                    <div className="terminal-row" key={event.sequence}>
                                        <span className="terminal-time">{new Date(event.captured_at_ms).toLocaleTimeString()}</span>
                                        <span className="terminal-meta">pid {event.pid} · uid {event.uid}</span>
                                        <span className="terminal-prompt">%</span>
                                        <span className="terminal-command">{event.command}</span>
                                    </div>
                                ))
                            )}
                        </div>
                    </div>

                    <div className="shell-callout-row">
                        <div>
                            <p className="trace-label">module selection</p>
                            <p className="trace-value">{shellConnectionState === "booting" ? "probing" : "--probe-module shell-commands"}</p>
                        </div>
                        <div>
                            <p className="trace-label">most used executable</p>
                            <p className="trace-value">{topExecutable}</p>
                        </div>
                        <div>
                            <p className="trace-label">history window</p>
                            <p className="trace-value">{SHELL_LOOKBACK_SECS}s / {MAX_SHELL_EVENTS} events</p>
                        </div>
                    </div>

                    {shellErrorMessage ? <p className="error-banner">{shellErrorMessage}</p> : null}
                </article>
            </section>

            <section className="overview-grid">
                <MetricCard
                    label="CPU"
                    value={currentSample ? `${currentSample.cpu_usage_pct.toFixed(1)}%` : "--"}
                    detail={currentSample ? `load ${currentSample.load_average_one.toFixed(2)}` : "waiting for samples"}
                />
                <MetricCard
                    label="Memory"
                    value={currentSample ? `${usedMemoryPct.toFixed(1)}%` : "--"}
                    detail={currentSample ? `${formatBytes(currentSample.used_memory_bytes)} of ${formatBytes(currentSample.total_memory_bytes)}` : "waiting for samples"}
                />
                <MetricCard
                    label="Disk"
                    value={currentSample ? `${diskUsedPct.toFixed(1)}%` : "--"}
                    detail={currentSample ? `${formatBytes(currentSample.available_disk_bytes)} free` : "waiting for samples"}
                />
                <MetricCard
                    label="Network burst"
                    value={currentSample ? `${formatBytes(currentSample.network_received_bytes + currentSample.network_transmitted_bytes)}/tick` : "--"}
                    detail={currentSample ? `${formatBytes(currentSample.total_network_received_bytes + currentSample.total_network_transmitted_bytes)} total` : "waiting for samples"}
                />
                <MetricCard label="trace queue" value={String(queueDepth)} detail={traceEnabled ? "raw events waiting to decode" : "trace disabled"} />
                <MetricCard label="last telemetry" value={telemetryFreshness} detail={history?.node.host_name ?? "waiting for host"} />
            </section>

            <section className="dashboard-grid">
                <article className="panel charts-panel">
                    <div className="panel-head">
                        <div>
                            <p className="eyebrow">live curves</p>
                            <h2>Recent pressure</h2>
                        </div>
                        <p className="microcopy">window: {deferredSamples.length} samples</p>
                    </div>
                    <div className="chart-stack">
                        <SparklineCard label="CPU usage" tone="warm" value={currentSample ? `${currentSample.cpu_usage_pct.toFixed(1)}%` : "--"} series={cpuSeries} />
                        <SparklineCard label="Memory pressure" tone="cool" value={currentSample ? `${usedMemoryPct.toFixed(1)}%` : "--"} series={memorySeries} />
                        <SparklineCard label="Network burst" tone="neutral" value={currentSample ? formatBytes(currentSample.network_received_bytes + currentSample.network_transmitted_bytes) : "--"} series={networkSeries} />
                    </div>
                </article>

                <article className="panel process-panel">
                    <div className="panel-head">
                        <div>
                            <p className="eyebrow">top processes</p>
                            <h2>Current workload</h2>
                        </div>
                        <p className="microcopy">sorted by the node before delivery</p>
                    </div>
                    <div className="process-list">
                        {topProcesses.length === 0 ? (
                            <p className="empty-state">No process snapshot yet.</p>
                        ) : (
                            topProcesses.map((process, index) => (
                                <div className="process-row" key={`${process.pid}-${index}`}>
                                    <div>
                                        <p className="process-name">{process.name}</p>
                                        <p className="process-meta">pid {process.pid} · {process.status}</p>
                                    </div>
                                    <div className="process-metrics">
                                        <span>{process.cpu_usage_pct.toFixed(1)}%</span>
                                        <span>{formatBytes(process.memory_bytes)}</span>
                                    </div>
                                </div>
                            ))
                        )}
                    </div>
                </article>

                <article className="panel detail-panel">
                    <div className="panel-head">
                        <div>
                            <p className="eyebrow">node profile</p>
                            <h2>Runtime details</h2>
                        </div>
                    </div>
                    <dl className="detail-grid">
                        <Detail label="system" value={history?.node.system_name ?? "--"} />
                        <Detail label="version" value={history?.node.os_version ?? "--"} />
                        <Detail label="kernel" value={history?.node.kernel_version ?? "--"} />
                        <Detail label="cpu layout" value={history ? `${history.node.cpu_count} logical / ${history.node.physical_core_count ?? "?"} physical` : "--"} />
                        <Detail label="retention" value={history ? `${history.retention_secs}s` : "--"} />
                        <Detail label="cadence" value={history ? `${history.sample_interval_ms}ms` : "--"} />
                        <Detail label="trace kind" value={traceSource} />
                        <Detail label="socket/program" value={traceHint} />
                    </dl>
                    {telemetryErrorMessage ? <p className="error-banner">{telemetryErrorMessage}</p> : null}
                </article>

                <article className="panel timeline-panel">
                    <div className="panel-head">
                        <div>
                            <p className="eyebrow">sample ledger</p>
                            <h2>Recent frames</h2>
                        </div>
                        <p className="microcopy">latest eight samples</p>
                    </div>
                    <div className="timeline-list">
                        {deferredSamples.slice(-8).reverse().map((sample) => (
                            <div className="timeline-row" key={sample.sequence}>
                                <span className="sequence">#{sample.sequence}</span>
                                <span>{new Date(sample.captured_at_ms).toLocaleTimeString()}</span>
                                <span>{sample.cpu_usage_pct.toFixed(1)}% cpu</span>
                                <span>{formatBytes(sample.used_memory_bytes)} ram</span>
                                <span>{sample.process_count} proc</span>
                            </div>
                        ))}
                    </div>
                </article>
            </section>
        </main>
    );
}

function KeyboardKeyCap(props: { keyDef: KeyDefinition; visualState: KeyVisualState | undefined }) {
    const width = props.keyDef.width ?? 1;
    const tone = props.keyDef.tone ?? "neutral";

    return (
        <div
            className={`keycap ${tone} ${props.visualState?.pressed ? "is-active" : ""} ${props.visualState?.pulsing ? "is-pulsing" : ""}`}
            style={{ "--key-width": String(width) } as CSSProperties}
            aria-label={props.keyDef.label}
        >
            {props.keyDef.secondary ? <span className="keycap-secondary">{props.keyDef.secondary}</span> : null}
            <span className="keycap-label">{props.keyDef.label}</span>
        </div>
    );
}

function MobileKeyPulseCard(props: { action: RenderedAction; nowMs: number }) {
    return (
        <div className={`mobile-key-card ${props.action.tone}`}>
            <span className="mobile-key-time">{formatFreshness(props.action.capturedAtMs, props.nowMs)}</span>
            <strong className="mobile-key-label">{props.action.label}</strong>
            <span className="mobile-key-tone">{props.action.tone}</span>
        </div>
    );
}

function MetricCard(props: { label: string; value: string; detail: string; compact?: boolean }) {
    return (
        <article className={`metric-card panel ${props.compact ? "compact" : ""}`}>
            <p className="eyebrow">{props.label}</p>
            <h2>{props.value}</h2>
            <p className="microcopy">{props.detail}</p>
        </article>
    );
}

function StatusPill(props: { label: string; value: string; detail?: string; accent: "warm" | "cool" | "neutral" }) {
    return (
        <div className={`status-pill ${props.accent}`}>
            <div className="status-pill-copy">
                <span className="status-pill-label">{props.label}</span>
                <strong>{props.value}</strong>
            </div>
            {props.detail ? <span className="status-pill-detail">{props.detail}</span> : null}
        </div>
    );
}

function TraceBadge(props: { label: string; value: string; tone: "warm" | "cool" | "neutral" }) {
    return (
        <div className={`trace-badge ${props.tone}`}>
            <span>{props.label}</span>
            <strong>{props.value}</strong>
        </div>
    );
}

function SparklineCard(props: { label: string; value: string; series: number[]; tone: "warm" | "cool" | "neutral" }) {
    return (
        <section className={`sparkline-card ${props.tone}`}>
            <div className="sparkline-head">
                <span>{props.label}</span>
                <strong>{props.value}</strong>
            </div>
            <Sparkline series={props.series} />
        </section>
    );
}

function Sparkline(props: { series: number[] }) {
    if (props.series.length === 0) {
        return <div className="sparkline-empty">waiting for data</div>;
    }

    const width = 320;
    const height = 88;
    const maxValue = Math.max(...props.series, 1);
    const minValue = Math.min(...props.series, 0);
    const range = Math.max(maxValue - minValue, 1);
    const step = props.series.length > 1 ? width / (props.series.length - 1) : width;

    const points = props.series
        .map((value, index) => {
            const x = index * step;
            const normalized = (value - minValue) / range;
            const y = height - normalized * height;
            return `${x},${y}`;
        })
        .join(" ");

    return (
        <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label="telemetry sparkline">
            <polyline points={points} />
        </svg>
    );
}

function Detail(props: { label: string; value: string }) {
    return (
        <div className="detail-item">
            <dt>{props.label}</dt>
            <dd>{props.value}</dd>
        </div>
    );
}

async function fetchJson<T>(url: string, signal?: AbortSignal): Promise<T> {
    const response = await fetch(url, { signal });

    if (!response.ok) {
        throw new Error(`${url} failed with ${response.status}`);
    }

    return (await response.json()) as T;
}

function buildKeyboardScene(events: KeyboardInputEvent[], nowMs: number): KeyboardScene {
    const keyStates = new Map<string, KeyVisualState>();
    const actions: RenderedAction[] = [];
    const transcriptTokens: TranscriptToken[] = [];

    for (const event of events) {
        keyStates.set(event.key_name, {
            pressed: event.state !== "up",
            pulsing: nowMs - event.captured_at_ms <= KEYBOARD_PULSE_MS,
        });

        if (event.state === "up") {
            continue;
        }

        const rendered = renderKeyboardAction(event);

        if (rendered.effect === "append") {
            appendTranscriptToken(transcriptTokens, event.sequence, rendered.text, rendered.tone);
        }

        if (rendered.effect === "backspace") {
            removeTranscriptToken(transcriptTokens);
        }

        trimTranscriptTokens(transcriptTokens);

        if (rendered.label) {
            actions.push({
                sequence: event.sequence,
                label: rendered.label,
                tone: rendered.tone,
                capturedAtMs: event.captured_at_ms,
            });
        }
    }

    const lines = splitTranscriptLines(transcriptTokens).slice(-9);
    const activeCount = [...keyStates.values()].filter((state) => state.pressed).length;
    const transcript = transcriptTokens
        .filter((token) => token.tone === "text")
        .map((token) => token.text)
        .join("");

    return {
        transcript,
        lines,
        actions: actions.slice(-14).reverse(),
        keyStates,
        activeCount,
    };
}

function renderKeyboardAction(event: KeyboardInputEvent): { effect: "append" | "backspace" | "none"; text: string; label: string; tone: TranscriptTone } {
    if (isModifierKey(event.key_name)) {
        return { effect: "none", text: "", label: "", tone: "navigation" };
    }

    if (event.modifiers.ctrl || event.modifiers.alt || event.modifiers.meta) {
        const chord = formatChord(event);
        const tone = classifyModifiedChord(event);
        return {
            effect: "append",
            text: `[${chord}]`,
            label: chord,
            tone,
        };
    }

    if (event.key_name === "KEY_BACKSPACE") {
        return { effect: "backspace", text: "", label: "Backspace", tone: "edit" };
    }

    if (event.key_name === "KEY_ENTER") {
        return { effect: "append", text: "\n", label: "Enter", tone: "edit" };
    }

    if (event.key_name === "KEY_TAB") {
        return { effect: "append", text: "    ", label: "Tab", tone: "edit" };
    }

    const printable = printableFromKey(event.key_name, event.modifiers.shift);

    if (printable !== null) {
        return {
            effect: "append",
            text: printable,
            label: printable === " " ? "Space" : printable,
            tone: "text",
        };
    }

    const label = humanizeKeyName(event.key_name);

    return {
        effect: "append",
        text: `[${label}]`,
        label,
        tone: "navigation",
    };
}

function appendTranscriptToken(tokens: TranscriptToken[], sequence: number, text: string, tone: TranscriptTone) {
    const previous = tokens[tokens.length - 1];

    if (tone === "text" && previous?.tone === "text") {
        previous.text += text;
        previous.id = `text-${sequence}`;
        return;
    }

    tokens.push({
        id: `${tone}-${sequence}-${tokens.length}`,
        text,
        tone,
    });
}

function removeTranscriptToken(tokens: TranscriptToken[]) {
    const previous = tokens[tokens.length - 1];

    if (!previous) {
        return;
    }

    if (previous.tone === "text" && previous.text.length > 1) {
        previous.text = previous.text.slice(0, -1);
        return;
    }

    tokens.pop();
}

function trimTranscriptTokens(tokens: TranscriptToken[]) {
    while (tokens.length > MAX_TRANSCRIPT_TOKENS) {
        tokens.shift();
    }

    let visibleTextLength = tokens
        .filter((token) => token.tone === "text")
        .reduce((sum, token) => sum + token.text.length, 0);

    while (visibleTextLength > MAX_TRANSCRIPT_CHARS && tokens.length > 0) {
        const first = tokens[0];

        if (first.tone !== "text") {
            tokens.shift();
            continue;
        }

        const overflow = visibleTextLength - MAX_TRANSCRIPT_CHARS;

        if (first.text.length <= overflow) {
            visibleTextLength -= first.text.length;
            tokens.shift();
            continue;
        }

        first.text = first.text.slice(overflow);
        visibleTextLength = MAX_TRANSCRIPT_CHARS;
    }
}

function splitTranscriptLines(tokens: TranscriptToken[]) {
    const lines: TranscriptToken[][] = [[]];

    for (const token of tokens) {
        const chunks = token.text.split("\n");

        chunks.forEach((chunk, index) => {
            if (chunk) {
                lines[lines.length - 1].push({
                    ...token,
                    id: `${token.id}-${index}`,
                    text: chunk,
                });
            }

            if (index < chunks.length - 1) {
                lines.push([]);
            }
        });
    }

    return lines;
}

function printableFromKey(keyName: string, shift: boolean): string | null {
    if (/^KEY_[A-Z]$/.test(keyName)) {
        const letter = keyName.slice(4);
        return shift ? letter : letter.toLowerCase();
    }

    const directMap: Record<string, [string, string]> = {
        KEY_1: ["1", "!"],
        KEY_2: ["2", "@"],
        KEY_3: ["3", "#"],
        KEY_4: ["4", "$"],
        KEY_5: ["5", "%"],
        KEY_6: ["6", "^"],
        KEY_7: ["7", "&"],
        KEY_8: ["8", "*"],
        KEY_9: ["9", "("],
        KEY_0: ["0", ")"],
        KEY_MINUS: ["-", "_"],
        KEY_EQUAL: ["=", "+"],
        KEY_LEFTBRACE: ["[", "{"],
        KEY_RIGHTBRACE: ["]", "}"],
        KEY_BACKSLASH: ["\\", "|"],
        KEY_SEMICOLON: [";", ":"],
        KEY_APOSTROPHE: ["'", '"'],
        KEY_GRAVE: ["`", "~"],
        KEY_COMMA: [",", "<"],
        KEY_DOT: [".", ">"],
        KEY_SLASH: ["/", "?"],
        KEY_SPACE: [" ", " "],
        KEY_KP0: ["0", "0"],
        KEY_KP1: ["1", "1"],
        KEY_KP2: ["2", "2"],
        KEY_KP3: ["3", "3"],
        KEY_KP4: ["4", "4"],
        KEY_KP5: ["5", "5"],
        KEY_KP6: ["6", "6"],
        KEY_KP7: ["7", "7"],
        KEY_KP8: ["8", "8"],
        KEY_KP9: ["9", "9"],
        KEY_KPDOT: [".", "."],
        KEY_KPSLASH: ["/", "/"],
        KEY_KPASTERISK: ["*", "*"],
        KEY_KPMINUS: ["-", "-"],
        KEY_KPPLUS: ["+", "+"],
    };

    const match = directMap[keyName];
    return match ? match[shift ? 1 : 0] : null;
}

function isModifierKey(keyName: string) {
    return [
        "KEY_LEFTCTRL",
        "KEY_RIGHTCTRL",
        "KEY_LEFTSHIFT",
        "KEY_RIGHTSHIFT",
        "KEY_LEFTALT",
        "KEY_RIGHTALT",
        "KEY_LEFTMETA",
        "KEY_RIGHTMETA",
        "KEY_CAPSLOCK",
    ].includes(keyName);
}

function formatChord(event: KeyboardInputEvent) {
    const parts: string[] = [];

    if (event.modifiers.ctrl) {
        parts.push("Ctrl");
    }

    if (event.modifiers.alt) {
        parts.push("Alt");
    }

    if (event.modifiers.meta) {
        parts.push("Super");
    }

    if (event.modifiers.shift && printableFromKey(event.key_name, event.modifiers.shift) === null) {
        parts.push("Shift");
    }

    parts.push(humanizeKeyName(event.key_name));
    return parts.join("+");
}

function classifyModifiedChord(event: KeyboardInputEvent): TranscriptTone {
    if (NAVIGATION_CHORD_KEYS.has(event.key_name)) {
        return "navigation";
    }

    if (event.modifiers.meta && SUPER_NAVIGATION_KEYS.has(event.key_name)) {
        return "navigation";
    }

    if (["KEY_BACKSPACE", "KEY_DELETE", "KEY_ENTER"].includes(event.key_name)) {
        return "edit";
    }

    return "command";
}

function humanizeKeyName(keyName: string) {
    const explicit: Record<string, string> = {
        KEY_ESC: "Esc",
        KEY_BACKSPACE: "Backspace",
        KEY_ENTER: "Enter",
        KEY_TAB: "Tab",
        KEY_CAPSLOCK: "Caps",
        KEY_LEFTSHIFT: "Shift",
        KEY_RIGHTSHIFT: "Shift",
        KEY_LEFTCTRL: "Ctrl",
        KEY_RIGHTCTRL: "Ctrl",
        KEY_LEFTALT: "Alt",
        KEY_RIGHTALT: "Alt",
        KEY_LEFTMETA: "Super",
        KEY_RIGHTMETA: "Super",
        KEY_SPACE: "Space",
        KEY_UP: "Up",
        KEY_DOWN: "Down",
        KEY_LEFT: "Left",
        KEY_RIGHT: "Right",
        KEY_MENU: "Menu",
        KEY_DELETE: "Delete",
    };

    if (explicit[keyName]) {
        return explicit[keyName];
    }

    return keyName
        .replace(/^KEY_/, "")
        .split("_")
        .map((part) => part.charAt(0) + part.slice(1).toLowerCase())
        .join(" ");
}

function countWords(text: string) {
    const trimmed = text.trim();

    if (!trimmed) {
        return 0;
    }

    return trimmed.split(/\s+/).length;
}

function percentage(part: number, total: number) {
    if (total === 0) {
        return 0;
    }

    return (part / total) * 100;
}

function formatBytes(value: number) {
    if (value === 0) {
        return "0 B";
    }

    const units = ["B", "KB", "MB", "GB", "TB"];
    let size = value;
    let unitIndex = 0;

    while (size >= 1024 && unitIndex < units.length - 1) {
        size /= 1024;
        unitIndex += 1;
    }

    return `${size.toFixed(size >= 10 || unitIndex === 0 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatFreshness(timestampMs: number, nowMs: number) {
    const delta = Math.max(nowMs - timestampMs, 0);
    return `${(delta / 1000).toFixed(delta >= 10_000 ? 0 : 1)}s ago`;
}

function connectionAccent(state: string): "warm" | "cool" | "neutral" {
    if (["live", "armed", "listening", "history ready"].includes(state)) {
        return "cool";
    }

    if (["reconnecting", "history failed"].includes(state)) {
        return "warm";
    }

    return "neutral";
}

function readerStateAccent(state: string): "warm" | "cool" | "neutral" {
    if (state === "running") {
        return "cool";
    }

    if (state === "backoff") {
        return "warm";
    }

    return "neutral";
}

function formatKeyboardSource(trace: KeyboardTraceStatusSnapshot) {
    if (!trace.enabled) {
        return "disabled";
    }

    if (trace.source_kind === "unix_socket") {
        return "socket sidecar";
    }

    if (trace.source_kind === "external_command") {
        return "direct trace";
    }

    return trace.source_kind;
}

function formatKeyboardProbe(trace: KeyboardTraceStatusSnapshot | null | undefined): ProbeDescriptor {
    const parsedProbe = trace ? parseProbeDescriptor(trace.args) : null;

    if (parsedProbe) {
        return parsedProbe;
    }

    return {
        kind: "kprobe",
        name: "input_event",
    };
}

function formatShellProbe(): ProbeDescriptor {
    return {
        kind: "uprobe",
        name: "zleread",
    };
}

function parseProbeDescriptor(args: string[]): ProbeDescriptor | null {
    for (const arg of args) {
        const match = arg.match(/\b(kprobe|uprobe|uretprobe):(?:[^:\s{}]+:)?([A-Za-z_][A-Za-z0-9_]*)/);

        if (!match) {
            continue;
        }

        return {
            kind: match[1] === "uretprobe" ? "uprobe" : match[1],
            name: match[2],
        };
    }

    return null;
}

function countRecentShellCommands(events: ShellCommandEvent[], nowMs: number, windowMs: number) {
    const cutoff = nowMs - windowMs;
    return events.filter((event) => event.captured_at_ms >= cutoff).length;
}

function mostExecutedBinary(events: ShellCommandEvent[]) {
    const counts = new Map<string, number>();

    for (const event of events) {
        const executable = event.executable ?? "(unknown)";
        counts.set(executable, (counts.get(executable) ?? 0) + 1);
    }

    let winner = "none";
    let highestCount = 0;

    for (const [executable, count] of counts) {
        if (count > highestCount) {
            winner = executable;
            highestCount = count;
        }
    }

    return winner;
}

function uniqueExecutables(events: ShellCommandEvent[]) {
    return new Set(events.map((event) => event.executable ?? event.command.split(/\s+/, 1)[0] ?? "(unknown)")).size;
}

export default App;