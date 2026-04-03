use std::{collections::VecDeque, time::Duration};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::{
    input_keys::{KeyboardEventBatch, KeyboardHistoryResponse, KeyboardInputEvent, PendingKeyboardEvent},
    protocol::{CollectedTelemetry, HistoryRequest, HistoryResponse, NodeMetadata, ProcessSummary, SystemSample},
};

const LIVE_CHANNEL_CAPACITY: usize = 256;
const KEYBOARD_LIVE_CHANNEL_CAPACITY: usize = 256;
const MIN_KEYBOARD_HISTORY_CAPACITY: usize = 1_024;

#[derive(Debug, Clone, Serialize)]
pub struct KeyboardStoreStatusSnapshot {
    pub retained_events: usize,
    pub dropped_events: u64,
    pub max_events: usize,
    pub next_sequence: u64,
    pub oldest_event_at_ms: Option<u64>,
    pub newest_event_at_ms: Option<u64>,
    pub live_subscribers: usize,
}

pub struct TelemetryStore {
    node: NodeMetadata,
    retention: Duration,
    sample_interval: Duration,
    max_samples: usize,
    next_sequence: u64,
    samples: VecDeque<SystemSample>,
    latest_processes: Vec<ProcessSummary>,
    live_sender: broadcast::Sender<CollectedTelemetry>,
    next_keyboard_sequence: u64,
    keyboard_events: VecDeque<KeyboardInputEvent>,
    keyboard_dropped_events: u64,
    keyboard_max_events: usize,
    keyboard_live_sender: broadcast::Sender<KeyboardEventBatch>,
}

impl TelemetryStore {
    pub fn new(
        node: NodeMetadata,
        retention: Duration,
        sample_interval: Duration,
        max_samples: usize,
    ) -> Self {
        let (live_sender, _) = broadcast::channel(LIVE_CHANNEL_CAPACITY);
        let (keyboard_live_sender, _) = broadcast::channel(KEYBOARD_LIVE_CHANNEL_CAPACITY);
        let keyboard_max_events = max_samples.saturating_mul(64).max(MIN_KEYBOARD_HISTORY_CAPACITY);

        Self {
            node,
            retention,
            sample_interval,
            max_samples,
            next_sequence: 1,
            samples: VecDeque::new(),
            latest_processes: Vec::new(),
            live_sender,
            next_keyboard_sequence: 1,
            keyboard_events: VecDeque::new(),
            keyboard_dropped_events: 0,
            keyboard_max_events,
            keyboard_live_sender,
        }
    }

    pub fn insert(&mut self, mut telemetry: CollectedTelemetry) {
        telemetry.sample.sequence = self.next_sequence;
        self.next_sequence += 1;

        self.latest_processes = telemetry.top_processes.clone();
        self.samples.push_back(telemetry.sample.clone());
        self.prune();

        let _ = self.live_sender.send(telemetry);
    }

    pub fn subscribe_live(&self) -> broadcast::Receiver<CollectedTelemetry> {
        self.live_sender.subscribe()
    }

    pub fn insert_keyboard_batch(
        &mut self,
        events: Vec<PendingKeyboardEvent>,
        published_at_ms: u64,
        dropped_events: u64,
    ) {
        self.keyboard_dropped_events = self.keyboard_dropped_events.saturating_add(dropped_events);

        if events.is_empty() {
            return;
        }

        let mut published_events = Vec::with_capacity(events.len());

        for event in events {
            let stored = KeyboardInputEvent {
                sequence: self.next_keyboard_sequence,
                captured_at_ms: event.captured_at_ms,
                device_ptr: event.device_ptr,
                key: event.key,
            };

            self.next_keyboard_sequence += 1;
            self.keyboard_events.push_back(stored.clone());
            published_events.push(stored);
        }

        self.prune_keyboard();

        let _ = self.keyboard_live_sender.send(KeyboardEventBatch {
            published_at_ms,
            dropped_events,
            events: published_events,
        });
    }

    pub fn subscribe_keyboard_live(&self) -> broadcast::Receiver<KeyboardEventBatch> {
        self.keyboard_live_sender.subscribe()
    }

    pub fn build_keyboard_history_response(
        &self,
        lookback_secs: Option<u64>,
        limit: Option<usize>,
        now_ms: u64,
    ) -> KeyboardHistoryResponse {
        let lookback_ms = lookback_secs.map(|secs| secs.saturating_mul(1_000));
        let earliest_ms = lookback_ms.map(|delta| now_ms.saturating_sub(delta));

        let mut events = self
            .keyboard_events
            .iter()
            .filter(|event| earliest_ms.is_none_or(|cutoff| event.captured_at_ms >= cutoff))
            .cloned()
            .collect::<Vec<_>>();

        if let Some(limit) = limit {
            let keep_from = events.len().saturating_sub(limit);
            events = events.split_off(keep_from);
        }

        KeyboardHistoryResponse {
            generated_at_ms: now_ms,
            retention_secs: self.retention.as_secs(),
            dropped_events: self.keyboard_dropped_events,
            events,
        }
    }

    pub fn build_keyboard_status_snapshot(&self) -> KeyboardStoreStatusSnapshot {
        KeyboardStoreStatusSnapshot {
            retained_events: self.keyboard_events.len(),
            dropped_events: self.keyboard_dropped_events,
            max_events: self.keyboard_max_events,
            next_sequence: self.next_keyboard_sequence,
            oldest_event_at_ms: self.keyboard_events.front().map(|event| event.captured_at_ms),
            newest_event_at_ms: self.keyboard_events.back().map(|event| event.captured_at_ms),
            live_subscribers: self.keyboard_live_sender.receiver_count(),
        }
    }

    pub fn build_history_response(&self, request: &HistoryRequest, now_ms: u64) -> HistoryResponse {
        let lookback_ms = request.lookback_secs.map(|secs| secs.saturating_mul(1_000));
        let earliest_ms = lookback_ms.map(|delta| now_ms.saturating_sub(delta));

        let samples = self
            .samples
            .iter()
            .filter(|sample| earliest_ms.is_none_or(|cutoff| sample.captured_at_ms >= cutoff))
            .cloned()
            .collect();

        let top_processes = if request.include_processes {
            let limit = request.max_processes.unwrap_or(self.latest_processes.len());
            self.latest_processes.iter().take(limit).cloned().collect()
        } else {
            Vec::new()
        };

        HistoryResponse {
            generated_at_ms: now_ms,
            retention_secs: self.retention.as_secs(),
            sample_interval_ms: self.sample_interval.as_millis() as u64,
            node: self.node.clone(),
            samples,
            top_processes,
        }
    }

    fn prune(&mut self) {
        while self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }

        if let Some(latest) = self.samples.back().map(|sample| sample.captured_at_ms) {
            let cutoff = latest.saturating_sub(self.retention.as_millis() as u64);
            while self
                .samples
                .front()
                .is_some_and(|sample| sample.captured_at_ms < cutoff)
            {
                self.samples.pop_front();
            }
        }
    }

    fn prune_keyboard(&mut self) {
        while self.keyboard_events.len() > self.keyboard_max_events {
            self.keyboard_events.pop_front();
        }

        if let Some(latest) = self.keyboard_events.back().map(|event| event.captured_at_ms) {
            let cutoff = latest.saturating_sub(self.retention.as_millis() as u64);
            while self
                .keyboard_events
                .front()
                .is_some_and(|event| event.captured_at_ms < cutoff)
            {
                self.keyboard_events.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TelemetryStore;
    use crate::{
        input_keys::{KeyboardKeyEvent, KeyState, Modifiers, PendingKeyboardEvent},
        protocol::{CollectedTelemetry, HistoryRequest, NodeMetadata, SystemSample},
    };
    use std::time::Duration;

    #[test]
    fn prunes_by_capacity_and_time() {
        let mut store = TelemetryStore::new(
            NodeMetadata {
                host_name: None,
                system_name: None,
                os_version: None,
                kernel_version: None,
                cpu_count: 1,
                physical_core_count: Some(1),
            },
            Duration::from_secs(2),
            Duration::from_secs(1),
            3,
        );

        for timestamp in [1_000_u64, 2_000, 3_000, 4_000] {
            store.insert(CollectedTelemetry {
                sample: SystemSample {
                    sequence: 0,
                    captured_at_ms: timestamp,
                    cpu_usage_pct: 0.0,
                    load_average_one: 0.0,
                    load_average_five: 0.0,
                    load_average_fifteen: 0.0,
                    total_memory_bytes: 0,
                    used_memory_bytes: 0,
                    available_memory_bytes: 0,
                    total_swap_bytes: 0,
                    used_swap_bytes: 0,
                    process_count: 0,
                    total_disk_bytes: 0,
                    available_disk_bytes: 0,
                    network_received_bytes: 0,
                    network_transmitted_bytes: 0,
                    total_network_received_bytes: 0,
                    total_network_transmitted_bytes: 0,
                },
                top_processes: Vec::new(),
            });
        }

        let response = store.build_history_response(&HistoryRequest::default(), 4_000);
        let timestamps: Vec<u64> = response.samples.into_iter().map(|sample| sample.captured_at_ms).collect();

        assert_eq!(timestamps, vec![2_000, 3_000, 4_000]);
    }

    #[test]
    fn broadcasts_live_updates() {
        let mut store = TelemetryStore::new(
            NodeMetadata {
                host_name: None,
                system_name: None,
                os_version: None,
                kernel_version: None,
                cpu_count: 1,
                physical_core_count: Some(1),
            },
            Duration::from_secs(2),
            Duration::from_secs(1),
            3,
        );

        let mut receiver = store.subscribe_live();

        store.insert(CollectedTelemetry {
            sample: SystemSample {
                sequence: 0,
                captured_at_ms: 1_000,
                cpu_usage_pct: 42.0,
                load_average_one: 0.0,
                load_average_five: 0.0,
                load_average_fifteen: 0.0,
                total_memory_bytes: 0,
                used_memory_bytes: 0,
                available_memory_bytes: 0,
                total_swap_bytes: 0,
                used_swap_bytes: 0,
                process_count: 0,
                total_disk_bytes: 0,
                available_disk_bytes: 0,
                network_received_bytes: 0,
                network_transmitted_bytes: 0,
                total_network_received_bytes: 0,
                total_network_transmitted_bytes: 0,
            },
            top_processes: Vec::new(),
        });

        let live = receiver.try_recv().expect("expected broadcast sample");
        assert_eq!(live.sample.sequence, 1);
        assert_eq!(live.sample.cpu_usage_pct, 42.0);
    }

    #[test]
    fn stores_keyboard_batches_for_history_and_live_streams() {
        let mut store = TelemetryStore::new(
            NodeMetadata {
                host_name: None,
                system_name: None,
                os_version: None,
                kernel_version: None,
                cpu_count: 1,
                physical_core_count: Some(1),
            },
            Duration::from_secs(5),
            Duration::from_secs(1),
            3,
        );

        let mut receiver = store.subscribe_keyboard_live();

        store.insert_keyboard_batch(
            vec![PendingKeyboardEvent {
                captured_at_ms: 1_000,
                device_ptr: Some("0xffff".to_owned()),
                key: KeyboardKeyEvent {
                    linux_code: 20,
                    key_name: "KEY_T".to_owned(),
                    state: KeyState::Down,
                    raw_scancode: Some(458775),
                    modifiers: Modifiers::default(),
                },
            }],
            1_005,
            2,
        );

        let batch = receiver.try_recv().expect("expected keyboard live batch");
        assert_eq!(batch.dropped_events, 2);
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].sequence, 1);

        let history = store.build_keyboard_history_response(None, None, 1_010);
        assert_eq!(history.dropped_events, 2);
        assert_eq!(history.events.len(), 1);
        assert_eq!(history.events[0].key.key_name, "KEY_T");
    }
}