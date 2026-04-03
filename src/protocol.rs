use serde::{Deserialize, Serialize};
use sysinfo::System;

pub const HISTORY_PROTOCOL: &str = "/watchmedo/history/1";
pub const LIVE_TELEMETRY_TOPIC: &str = "watchmedo/live/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRequest {
    pub lookback_secs: Option<u64>,
    pub include_processes: bool,
    pub max_processes: Option<usize>,
}

impl Default for HistoryRequest {
    fn default() -> Self {
        Self {
            lookback_secs: None,
            include_processes: true,
            max_processes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub generated_at_ms: u64,
    pub retention_secs: u64,
    pub sample_interval_ms: u64,
    pub node: NodeMetadata,
    pub samples: Vec<SystemSample>,
    pub top_processes: Vec<ProcessSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadata {
    pub host_name: Option<String>,
    pub system_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    pub cpu_count: usize,
    pub physical_core_count: Option<usize>,
}

impl NodeMetadata {
    pub fn capture() -> Self {
        let system = System::new_all();

        Self {
            host_name: System::host_name(),
            system_name: System::name(),
            os_version: System::os_version(),
            kernel_version: System::kernel_version(),
            cpu_count: system.cpus().len(),
            physical_core_count: System::physical_core_count(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSample {
    pub sequence: u64,
    pub captured_at_ms: u64,
    pub cpu_usage_pct: f32,
    pub load_average_one: f64,
    pub load_average_five: f64,
    pub load_average_fifteen: f64,
    pub total_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub total_swap_bytes: u64,
    pub used_swap_bytes: u64,
    pub process_count: usize,
    pub total_disk_bytes: u64,
    pub available_disk_bytes: u64,
    pub network_received_bytes: u64,
    pub network_transmitted_bytes: u64,
    pub total_network_received_bytes: u64,
    pub total_network_transmitted_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub pid: String,
    pub name: String,
    pub cpu_usage_pct: f32,
    pub memory_bytes: u64,
    pub virtual_memory_bytes: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectedTelemetry {
    pub sample: SystemSample,
    pub top_processes: Vec<ProcessSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTelemetryEvent {
    pub published_at_ms: u64,
    pub sample: SystemSample,
    pub top_processes: Vec<ProcessSummary>,
}

impl CollectedTelemetry {
    pub fn into_live_event(self, published_at_ms: u64) -> LiveTelemetryEvent {
        LiveTelemetryEvent {
            published_at_ms,
            sample: self.sample,
            top_processes: self.top_processes,
        }
    }
}