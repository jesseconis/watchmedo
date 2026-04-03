use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::Method,
    response::{IntoResponse, Sse, sse::{Event, KeepAlive}},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use tokio::{net::TcpListener, signal, sync::RwLock};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use crate::{
    input_keys::KeyboardHistoryResponse,
    input_trace::{KeyboardTraceHandle, KeyboardTraceStatusSnapshot},
    protocol::{HistoryRequest, LiveTelemetryEvent},
    shell_history::ShellHistoryResponse,
    state::{KeyboardStoreStatusSnapshot, ShellStoreStatusSnapshot, TelemetryStore},
};

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub listen_addr: SocketAddr,
    pub keyboard_trace: Option<KeyboardTraceHandle>,
}

#[derive(Clone)]
struct WebState {
    store: Arc<RwLock<TelemetryStore>>,
    keyboard_trace: Option<KeyboardTraceHandle>,
}

#[derive(Debug, serde::Serialize)]
struct KeyboardStatusResponse {
    timestamp_ms: u64,
    trace: KeyboardTraceStatusSnapshot,
    store: KeyboardStoreStatusSnapshot,
}

#[derive(Debug, serde::Serialize)]
struct ShellStatusResponse {
    timestamp_ms: u64,
    store: ShellStoreStatusSnapshot,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryQuery {
    lookback_secs: Option<u64>,
    include_processes: Option<bool>,
    max_processes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardHistoryQuery {
    lookback_secs: Option<u64>,
    max_events: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellHistoryQuery {
    lookback_secs: Option<u64>,
    max_events: Option<usize>,
}

pub async fn run(state: Arc<RwLock<TelemetryStore>>, config: WebConfig) -> anyhow::Result<()> {
    let WebConfig {
        listen_addr,
        keyboard_trace,
    } = config;

    let app = Router::new()
        .route("/api/healthz", get(health_handler))
        .route("/api/history", get(history_handler))
        .route("/api/live", get(live_handler))
        .route("/api/keyboard/history", get(keyboard_history_handler))
        .route("/api/keyboard/live", get(keyboard_live_handler))
        .route("/api/keyboard/status", get(keyboard_status_handler))
        .route("/api/shell/history", get(shell_history_handler))
        .route("/api/shell/live", get(shell_live_handler))
        .route("/api/shell/status", get(shell_status_handler))
        .with_state(WebState {
            store: state,
            keyboard_trace,
        })
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_headers(Any)
                .allow_methods([Method::GET]),
        );

    let listener = TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("failed to bind web api to {}", listen_addr))?;

    info!(address = %listen_addr, "watchmedo web api started");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
            info!("web api shutdown signal received");
        })
        .await
        .context("web api server failed")
}

async fn health_handler() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "timestampMs": now_ms(),
    }))
}

async fn history_handler(
    State(state): State<WebState>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let request = HistoryRequest {
        lookback_secs: query.lookback_secs,
        include_processes: query.include_processes.unwrap_or(true),
        max_processes: query.max_processes,
    };

    let response = {
        let guard = state.store.read().await;
        guard.build_history_response(&request, now_ms())
    };

    Json(response)
}

async fn live_handler(State(state): State<WebState>) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = {
        let guard = state.store.read().await;
        guard.subscribe_live()
    };

    let event_stream = stream! {
        yield Ok(
            Event::default()
                .event("ready")
                .data("connected")
        );

        loop {
            match receiver.recv().await {
                Ok(telemetry) => {
                    let payload = LiveTelemetryEvent {
                        published_at_ms: now_ms(),
                        sample: telemetry.sample,
                        top_processes: telemetry.top_processes,
                    };

                    match serde_json::to_string(&payload) {
                        Ok(json) => {
                            yield Ok(
                                Event::default()
                                    .event("telemetry")
                                    .data(json)
                            );
                        }
                        Err(error) => {
                            warn!(?error, "failed to serialize live event for web api");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "web api live stream lagged behind the collector");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    )
}

async fn keyboard_history_handler(
    State(state): State<WebState>,
    Query(query): Query<KeyboardHistoryQuery>,
) -> Json<KeyboardHistoryResponse> {
    let response = {
        let guard = state.store.read().await;
        guard.build_keyboard_history_response(query.lookback_secs, query.max_events, now_ms())
    };

    Json(response)
}

async fn keyboard_live_handler(
    State(state): State<WebState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = {
        let guard = state.store.read().await;
        guard.subscribe_keyboard_live()
    };

    let event_stream = stream! {
        yield Ok(
            Event::default()
                .event("ready")
                .data("connected")
        );

        loop {
            match receiver.recv().await {
                Ok(batch) => {
                    match serde_json::to_string(&batch) {
                        Ok(json) => {
                            yield Ok(
                                Event::default()
                                    .event("keyboard")
                                    .data(json)
                            );
                        }
                        Err(error) => {
                            warn!(?error, "failed to serialize keyboard batch for web api");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "web api keyboard stream lagged behind the keyboard trace pipeline");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    )
}

async fn keyboard_status_handler(State(state): State<WebState>) -> Json<KeyboardStatusResponse> {
    let store = {
        let guard = state.store.read().await;
        guard.build_keyboard_status_snapshot()
    };

    let trace = match &state.keyboard_trace {
        Some(handle) => handle.snapshot().await,
        None => KeyboardTraceStatusSnapshot::disabled(),
    };

    Json(KeyboardStatusResponse {
        timestamp_ms: now_ms(),
        trace,
        store,
    })
}

async fn shell_history_handler(
    State(state): State<WebState>,
    Query(query): Query<ShellHistoryQuery>,
) -> Json<ShellHistoryResponse> {
    let response = {
        let guard = state.store.read().await;
        guard.build_shell_history_response(query.lookback_secs, query.max_events, now_ms())
    };

    Json(response)
}

async fn shell_live_handler(
    State(state): State<WebState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = {
        let guard = state.store.read().await;
        guard.subscribe_shell_live()
    };

    let event_stream = stream! {
        yield Ok(
            Event::default()
                .event("ready")
                .data("connected")
        );

        loop {
            match receiver.recv().await {
                Ok(batch) => {
                    match serde_json::to_string(&batch) {
                        Ok(json) => {
                            yield Ok(
                                Event::default()
                                    .event("shell")
                                    .data(json)
                            );
                        }
                        Err(error) => {
                            warn!(?error, "failed to serialize shell batch for web api");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "web api shell stream lagged behind the shell trace pipeline");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    )
}

async fn shell_status_handler(State(state): State<WebState>) -> Json<ShellStatusResponse> {
    let store = {
        let guard = state.store.read().await;
        guard.build_shell_status_snapshot()
    };

    Json(ShellStatusResponse {
        timestamp_ms: now_ms(),
        store,
    })
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}