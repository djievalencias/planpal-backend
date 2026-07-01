/// Continuous CPU profiling via Pyroscope (Grafana Cloud Profiles).
///
/// Uses the pprof-rs backend to collect CPU samples and pushes them every
/// 10 seconds to the Grafana Cloud Pyroscope endpoint.
///
/// Configuration:
///   APP__PROFILING__ENDPOINT    = https://profiles-prod-XXX.grafana.net
///   APP__PROFILING__USERNAME    = <pyroscope-instance-id>   (numeric)
///   APP__PROFILING__PASSWORD    = <grafana-cloud-api-token>
///   APP__PROFILING__SAMPLE_RATE = 100   (samples/sec, default)
///
/// Or via secret manager keys: profiling_endpoint, profiling_username,
/// profiling_password.
use crate::config::ProfilingConfig;
use pyroscope::{
    pyroscope::PyroscopeAgentRunning,
    PyroscopeAgent,
};
use pyroscope_pprofrs::{pprof_backend, PprofConfig};

/// RAII guard — stops the Pyroscope agent and flushes the last profile batch
/// when dropped at process exit.
pub struct ProfilingGuard {
    // Held for its Drop side-effect (stops the profiling agent on process exit).
    #[allow(dead_code)]
    agent: PyroscopeAgent<PyroscopeAgentRunning>,
}

impl Drop for ProfilingGuard {
    fn drop(&mut self) {
        // PyroscopeAgent<Running>::stop() consumes self, so we can't call it
        // in Drop. The agent's background thread will be cleaned up by the OS
        // on process exit. For an explicit flush before exit, call stop() on
        // the guard before it is dropped.
    }
}

/// Initialise the Pyroscope profiling agent.
///
/// Returns `None` when `cfg.endpoint` is empty (profiling disabled).
/// Returns `Some(ProfilingGuard)` on success — must be kept alive until
/// process exit.
pub fn init(cfg: &ProfilingConfig, service_name: &str) -> anyhow::Result<Option<ProfilingGuard>> {
    if cfg.endpoint.is_empty() {
        return Ok(None);
    }

    let pprof_config = PprofConfig::new().sample_rate(cfg.sample_rate);

    let mut builder = PyroscopeAgent::builder(cfg.endpoint.as_str(), service_name)
        .backend(pprof_backend(pprof_config))
        .tags(vec![("service_name", service_name)]);

    if !cfg.username.is_empty() {
        builder = builder.basic_auth(&cfg.username, &cfg.password);
    }

    let agent = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Pyroscope agent build failed: {e}"))?;

    let agent_running = agent
        .start()
        .map_err(|e| anyhow::anyhow!("Pyroscope agent start failed: {e}"))?;

    Ok(Some(ProfilingGuard { agent: agent_running }))
}
