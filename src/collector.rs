use std::{cmp::Ordering, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};

use anyhow::Context;
use sysinfo::{Disks, MINIMUM_CPU_UPDATE_INTERVAL, Networks, ProcessesToUpdate, System};
use tokio::{sync::RwLock, task::JoinHandle, time::{self, MissedTickBehavior}};
use tracing::{debug, warn};

use crate::{protocol::{CollectedTelemetry, ProcessSummary, SystemSample}, state::TelemetryStore};

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    pub interval: Duration,
    pub top_processes: usize,
}

pub fn spawn_collector(
    state: Arc<RwLock<TelemetryStore>>,
    config: CollectorConfig,
) -> anyhow::Result<JoinHandle<()>> {
    let _ = config
        .interval
        .checked_add(MINIMUM_CPU_UPDATE_INTERVAL)
        .context("collector interval overflow")?;

    Ok(tokio::spawn(async move {
        let mut system = System::new_all();
        let mut networks = Networks::new_with_refreshed_list();
        let mut disks = Disks::new_with_refreshed_list();

        time::sleep(MINIMUM_CPU_UPDATE_INTERVAL).await;

        let mut ticker = time::interval(config.interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;

            let telemetry = collect_once(&mut system, &mut networks, &mut disks, config.top_processes);
            let captured_at_ms = telemetry.sample.captured_at_ms;

            {
                let mut guard = state.write().await;
                guard.insert(telemetry);
            }

            debug!(captured_at_ms, "collected telemetry sample");
        }
    }))
}

fn collect_once(
    system: &mut System,
    networks: &mut Networks,
    disks: &mut Disks,
    top_processes: usize,
) -> CollectedTelemetry {
    system.refresh_cpu_usage();
    system.refresh_memory();
    system.refresh_processes(ProcessesToUpdate::All, true);
    networks.refresh(true);
    disks.refresh(true);

    let load = System::load_average();
    let captured_at_ms = now_ms();

    let (network_received_bytes, total_network_received_bytes) = networks
        .iter()
        .fold((0_u64, 0_u64), |(received, total), (_, data)| {
            (
                received.saturating_add(data.received()),
                total.saturating_add(data.total_received()),
            )
        });
    let (network_transmitted_bytes, total_network_transmitted_bytes) = networks
        .iter()
        .fold((0_u64, 0_u64), |(transmitted, total), (_, data)| {
            (
                transmitted.saturating_add(data.transmitted()),
                total.saturating_add(data.total_transmitted()),
            )
        });

    let (total_disk_bytes, available_disk_bytes) = disks.iter().fold((0_u64, 0_u64), |acc, disk| {
        (
            acc.0.saturating_add(disk.total_space()),
            acc.1.saturating_add(disk.available_space()),
        )
    });

    let mut processes = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessSummary {
            pid: pid.to_string(),
            name: process.name().to_string_lossy().into_owned(),
            cpu_usage_pct: process.cpu_usage(),
            memory_bytes: process.memory(),
            virtual_memory_bytes: process.virtual_memory(),
            status: format!("{:?}", process.status()),
        })
        .collect::<Vec<_>>();

    processes.sort_by(|left, right| {
        right
            .cpu_usage_pct
            .partial_cmp(&left.cpu_usage_pct)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.memory_bytes.cmp(&left.memory_bytes))
            .then_with(|| left.name.cmp(&right.name))
    });
    processes.truncate(top_processes);

    CollectedTelemetry {
        sample: SystemSample {
            sequence: 0,
            captured_at_ms,
            cpu_usage_pct: system.global_cpu_usage(),
            load_average_one: load.one,
            load_average_five: load.five,
            load_average_fifteen: load.fifteen,
            total_memory_bytes: system.total_memory(),
            used_memory_bytes: system.used_memory(),
            available_memory_bytes: system.available_memory(),
            total_swap_bytes: system.total_swap(),
            used_swap_bytes: system.used_swap(),
            process_count: system.processes().len(),
            total_disk_bytes,
            available_disk_bytes,
            network_received_bytes,
            network_transmitted_bytes,
            total_network_received_bytes,
            total_network_transmitted_bytes,
        },
        top_processes: processes,
    }
}

fn now_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(error) => {
            warn!(?error, "system clock appears to be before unix epoch");
            0
        }
    }
}