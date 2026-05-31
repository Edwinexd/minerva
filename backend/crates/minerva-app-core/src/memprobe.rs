//! Periodic `/proc/self/status` memory probe, shared by every service
//! binary (api / worker / scheduler / model servers). Each binary calls
//! [`spawn`] from its own `main`, so the emitted `memprobe: ...` trace line
//! is tagged with that binary's log target and a grep lands on the right
//! pod. First thing to reach for when the next memory incident lands.

/// Spawn the probe task: 5 s interval for the first 5 min of process life
/// (the fragile startup window where the embedder cache is filling), 60 s
/// thereafter.
pub fn spawn() {
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        loop {
            if let Some(stats) = read_proc_self_status() {
                tracing::info!(
                    "memprobe: uptime={}s vm_rss={} MiB vm_hwm={} MiB vm_size={} MiB vm_data={} MiB threads={}",
                    started.elapsed().as_secs(),
                    stats.vm_rss_kb / 1024,
                    stats.vm_hwm_kb / 1024,
                    stats.vm_size_kb / 1024,
                    stats.vm_data_kb / 1024,
                    stats.threads,
                );
            }
            let interval = if started.elapsed() < std::time::Duration::from_secs(5 * 60) {
                std::time::Duration::from_secs(5)
            } else {
                std::time::Duration::from_secs(60)
            };
            tokio::time::sleep(interval).await;
        }
    });
}

/// Selected fields from `/proc/self/status`, all in KiB / counts. Used by
/// the periodic [`spawn`] task to give us a trace of process memory and
/// thread count right up to an OOM kill, so the next incident points at
/// the actual offender instead of needing a guess. Only Linux pods set
/// these; on a non-Linux dev host this returns `None` and the probe
/// silently skips.
struct ProcStatus {
    vm_rss_kb: u64,
    vm_hwm_kb: u64,
    vm_size_kb: u64,
    vm_data_kb: u64,
    threads: u64,
}

fn read_proc_self_status() -> Option<ProcStatus> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut vm_rss_kb = None;
    let mut vm_hwm_kb = None;
    let mut vm_size_kb = None;
    let mut vm_data_kb = None;
    let mut threads = None;
    for line in content.lines() {
        let parse_kb = |rest: &str| -> Option<u64> {
            rest.split_whitespace().next().and_then(|s| s.parse().ok())
        };
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            vm_rss_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            vm_hwm_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmSize:") {
            vm_size_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmData:") {
            vm_data_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = parse_kb(rest);
        }
    }
    Some(ProcStatus {
        vm_rss_kb: vm_rss_kb?,
        vm_hwm_kb: vm_hwm_kb?,
        vm_size_kb: vm_size_kb?,
        vm_data_kb: vm_data_kb?,
        threads: threads?,
    })
}
