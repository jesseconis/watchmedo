use std::{fmt, sync::Arc, time::Duration};

use anyhow::Context;
use futures::StreamExt;
use libp2p::{
    Multiaddr, StreamProtocol, SwarmBuilder,
    gossipsub, identity, noise, request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::{signal, sync::{broadcast, RwLock}};
use tracing::{debug, info, warn};

use crate::{
    protocol::{CollectedTelemetry, HISTORY_PROTOCOL, HistoryRequest, HistoryResponse, LIVE_TELEMETRY_TOPIC},
    state::TelemetryStore,
};

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "WatchmedoBehaviourEvent")]
struct WatchmedoBehaviour {
    request_response: request_response::json::Behaviour<HistoryRequest, HistoryResponse>,
    gossipsub: gossipsub::Behaviour,
}

enum WatchmedoBehaviourEvent {
    RequestResponse(request_response::Event<HistoryRequest, HistoryResponse>),
    Gossipsub(gossipsub::Event),
}

impl From<request_response::Event<HistoryRequest, HistoryResponse>> for WatchmedoBehaviourEvent {
    fn from(event: request_response::Event<HistoryRequest, HistoryResponse>) -> Self {
        Self::RequestResponse(event)
    }
}

impl From<gossipsub::Event> for WatchmedoBehaviourEvent {
    fn from(event: gossipsub::Event) -> Self {
        Self::Gossipsub(event)
    }
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub listen_addr: Multiaddr,
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub connect_addr: Multiaddr,
    pub history_request: HistoryRequest,
}

pub async fn run_server(
    state: Arc<RwLock<TelemetryStore>>,
    config: NetworkConfig,
) -> anyhow::Result<()> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = local_key.public().to_peer_id();

    let request_response = request_response::json::Behaviour::<HistoryRequest, HistoryResponse>::new(
        [(StreamProtocol::new(HISTORY_PROTOCOL), request_response::ProtocolSupport::Full)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
    );

    let live_topic = gossipsub::IdentTopic::new(LIVE_TELEMETRY_TOPIC);
    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub::ConfigBuilder::default()
            .build()
            .context("failed to build gossipsub config")?,
    )
    .map_err(|error| anyhow::anyhow!("failed to build gossipsub behaviour: {error}"))?;

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )
        .context("failed to build tcp transport")?
        .with_behaviour(|_| WatchmedoBehaviour {
            request_response,
            gossipsub,
        })
        .context("failed to build swarm behaviour")?
        .build();

    let mut live_receiver = {
        let guard = state.read().await;
        guard.subscribe_live()
    };

    swarm
        .listen_on(config.listen_addr)
        .context("failed to start libp2p listener")?;

    info!(peer_id = %local_peer_id, history_protocol = HISTORY_PROTOCOL, live_topic = LIVE_TELEMETRY_TOPIC, "watchmedo node started");

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutdown signal received");
                return Ok(());
            }
            live_update = live_receiver.recv() => {
                match live_update {
                    Ok(telemetry) => publish_live_update(&mut swarm, &live_topic, telemetry),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "live telemetry publisher lagged behind the collector");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        warn!("live telemetry channel closed");
                        return Ok(());
                    }
                }
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(%address, "listening for telemetry requests");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        info!(%peer_id, "peer connected");
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        info!(%peer_id, ?cause, "peer disconnected");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                let now_ms = current_time_ms();
                                let response = {
                                    let guard = state.read().await;
                                    guard.build_history_response(&request, now_ms)
                                };

                                if swarm.behaviour_mut().request_response.send_response(channel, response).is_err() {
                                    warn!(%peer, "failed to send telemetry history response");
                                }
                            }
                            request_response::Message::Response { request_id, .. } => {
                                info!(?request_id, %peer, "received response on server node");
                            }
                        }
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                        warn!(%peer, ?request_id, ?error, "outbound request failure");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::InboundFailure { peer, request_id, error, .. })) => {
                        warn!(%peer, ?request_id, ?error, "inbound request failure");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::ResponseSent { peer, request_id, .. })) => {
                        info!(%peer, ?request_id, "history response sent");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source,
                        message_id,
                        message,
                    })) => {
                        debug!(
                            %propagation_source,
                            ?message_id,
                            payload_bytes = message.data.len(),
                            "received live telemetry topic message"
                        );
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic })) => {
                        info!(%peer_id, topic = %topic, "peer subscribed to live telemetry");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic })) => {
                        info!(%peer_id, topic = %topic, "peer unsubscribed from live telemetry");
                    }
                    _ => {}
                }
            }
        }
    }
}

pub async fn run_client(config: ClientConfig) -> anyhow::Result<()> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = local_key.public().to_peer_id();
    let live_topic = gossipsub::IdentTopic::new(LIVE_TELEMETRY_TOPIC);

    let mut swarm = build_swarm(local_key).await?;
    swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&live_topic)
        .map_err(|error| anyhow::anyhow!("failed to subscribe to live telemetry topic: {error}"))?;

    swarm
        .dial(config.connect_addr.clone())
        .context("failed to dial telemetry node")?;

    info!(peer_id = %local_peer_id, connect_addr = %config.connect_addr, live_topic = LIVE_TELEMETRY_TOPIC, "watchmedo client started");

    let mut target_peer = None;
    let mut requested_history = false;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutdown signal received");
                return Ok(());
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        info!(%peer_id, "connected to telemetry node");
                        target_peer = Some(peer_id);

                        if !requested_history {
                            let request_id = swarm
                                .behaviour_mut()
                                .request_response
                                .send_request(&peer_id, config.history_request.clone());
                            requested_history = true;
                            info!(?request_id, lookback_secs = ?config.history_request.lookback_secs, include_processes = config.history_request.include_processes, "requested telemetry history");
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        warn!(%peer_id, ?cause, "connection to telemetry node closed");
                        if target_peer == Some(peer_id) {
                            requested_history = false;
                        }
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                        match message {
                            request_response::Message::Response { request_id, response } => {
                                info!(?request_id, %peer, samples = response.samples.len(), processes = response.top_processes.len(), "received telemetry history");
                                log_history_response(&response);
                            }
                            request_response::Message::Request { channel, .. } => {
                                warn!(%peer, "client received unexpected inbound request");
                                let empty = HistoryResponse {
                                    generated_at_ms: current_time_ms(),
                                    retention_secs: 0,
                                    sample_interval_ms: 0,
                                    node: crate::protocol::NodeMetadata {
                                        host_name: None,
                                        system_name: None,
                                        os_version: None,
                                        kernel_version: None,
                                        cpu_count: 0,
                                        physical_core_count: None,
                                    },
                                    samples: Vec::new(),
                                    top_processes: Vec::new(),
                                };

                                if swarm.behaviour_mut().request_response.send_response(channel, empty).is_err() {
                                    warn!(%peer, "failed to reject unexpected inbound request");
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                        warn!(%peer, ?request_id, ?error, "history request failed");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::InboundFailure { peer, request_id, error, .. })) => {
                        warn!(%peer, ?request_id, ?error, "client inbound failure");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::RequestResponse(request_response::Event::ResponseSent { peer, request_id, .. })) => {
                        debug!(%peer, ?request_id, "client sent a response unexpectedly");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Message { propagation_source, message_id, message })) => {
                        match serde_json::from_slice::<crate::protocol::LiveTelemetryEvent>(&message.data) {
                            Ok(event) => {
                                info!(%propagation_source, ?message_id, sequence = event.sample.sequence, cpu_usage_pct = event.sample.cpu_usage_pct, used_memory_bytes = event.sample.used_memory_bytes, processes = event.top_processes.len(), "received live telemetry event");
                            }
                            Err(error) => {
                                warn!(%propagation_source, ?message_id, ?error, "failed to decode live telemetry event");
                            }
                        }
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic })) => {
                        info!(%peer_id, topic = %topic, "peer joined live telemetry topic");
                    }
                    SwarmEvent::Behaviour(WatchmedoBehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic })) => {
                        info!(%peer_id, topic = %topic, "peer left live telemetry topic");
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn build_swarm(local_key: identity::Keypair) -> anyhow::Result<libp2p::Swarm<WatchmedoBehaviour>> {
    let request_response = request_response::json::Behaviour::<HistoryRequest, HistoryResponse>::new(
        [(StreamProtocol::new(HISTORY_PROTOCOL), request_response::ProtocolSupport::Full)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
    );

    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub::ConfigBuilder::default()
            .build()
            .context("failed to build gossipsub config")?,
    )
    .map_err(|error| anyhow::anyhow!("failed to build gossipsub behaviour: {error}"))?;

    Ok(
        SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )
        .context("failed to build tcp transport")?
        .with_behaviour(|_| WatchmedoBehaviour {
            request_response,
            gossipsub,
        })
        .context("failed to build swarm behaviour")?
        .build(),
    )
}

fn log_history_response(response: &HistoryResponse) {
    let latest = response.samples.last();
    let host_name = response.node.host_name.as_deref().unwrap_or("unknown");

    info!(host_name, retention_secs = response.retention_secs, sample_interval_ms = response.sample_interval_ms, "history metadata");

    if let Some(sample) = latest {
        info!(
            sequence = sample.sequence,
            captured_at_ms = sample.captured_at_ms,
            cpu_usage_pct = sample.cpu_usage_pct,
            used_memory_bytes = sample.used_memory_bytes,
            available_memory_bytes = sample.available_memory_bytes,
            network_received_bytes = sample.network_received_bytes,
            network_transmitted_bytes = sample.network_transmitted_bytes,
            "latest history sample"
        );
    }

    for process in response.top_processes.iter().take(5) {
        info!(
            pid = %process.pid,
            name = %process.name,
            cpu_usage_pct = process.cpu_usage_pct,
            memory_bytes = process.memory_bytes,
            status = %process.status,
            "top process"
        );
    }
}

fn publish_live_update(
    swarm: &mut libp2p::Swarm<WatchmedoBehaviour>,
    live_topic: &gossipsub::IdentTopic,
    telemetry: CollectedTelemetry,
) {
    let payload = match serde_json::to_vec(&telemetry.into_live_event(current_time_ms())) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(?error, "failed to serialize live telemetry update");
            return;
        }
    };

    if let Err(error) = swarm.behaviour_mut().gossipsub.publish(live_topic.clone(), payload) {
        debug!(?error, topic = LIVE_TELEMETRY_TOPIC, "live telemetry publish skipped");
    }
}

impl fmt::Display for WatchmedoBehaviourEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestResponse(_) => formatter.write_str("request-response"),
            Self::Gossipsub(_) => formatter.write_str("gossipsub"),
        }
    }
}

fn current_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}