use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingShellCommandEvent {
    pub captured_at_ms: u64,
    pub pid: u32,
    pub uid: u32,
    pub command: String,
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandEvent {
    pub sequence: u64,
    pub captured_at_ms: u64,
    pub pid: u32,
    pub uid: u32,
    pub command: String,
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandBatch {
    pub published_at_ms: u64,
    pub events: Vec<ShellCommandEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellHistoryResponse {
    pub generated_at_ms: u64,
    pub retention_secs: u64,
    pub events: Vec<ShellCommandEvent>,
}