/// Prometheus metrics for the PlanPal application.
///
/// ─── 4 Golden Signals ────────────────────────────────────────────────────────
///
///  1. Latency    → `http_request_duration_seconds`  (histogram, per route)
///  2. Traffic    → `http_requests_total`             (counter, per route)
///  3. Errors     → `http_errors_total`               (counter, 4xx + 5xx)
///  4. Saturation → `db_pool_connections_{active,idle,max}` (gauges)
///                  `process_*` from ProcessCollector (cpu, memory, fds)
///
/// ─── 10 Additional App-Level Metrics ─────────────────────────────────────────
///
///  5.  `process_cpu_seconds_total`       — CPU time consumed by this process
///  6.  `process_resident_memory_bytes`   — RSS memory (process collector)
///  7.  `db_pool_connections_active`      — in-use DB connections
///  8.  `db_pool_connections_idle`        — idle DB connections in pool
///  9.  `db_pool_connections_max`         — configured pool ceiling
/// 10.  `http_request_size_bytes`         — request body size histogram
/// 11.  `nats_messages_published_total`   — NATS publishes by job type
/// 12.  `nats_publish_errors_total`       — failed NATS publishes by job type
/// 13.  `worker_jobs_processed_total`     — jobs by worker / type / outcome
/// 14.  `email_sends_total`               — SMTP sends by status
/// 15.  `push_notification_sends_total`   — FCM sends by event / status
/// 16.  `redis_operations_total`          — Redis ops by operation / status
use prometheus::{
    register_counter_vec_with_registry, register_gauge_with_registry,
    register_histogram_vec_with_registry, CounterVec, Gauge, HistogramVec, Registry, TextEncoder,
    Encoder,
};

pub struct Metrics {
    // ── Golden Signal 1: Latency ──────────────────────────────────────────────
    pub http_request_duration_seconds: HistogramVec,

    // ── Golden Signal 2: Traffic ──────────────────────────────────────────────
    pub http_requests_total: CounterVec,

    // ── Golden Signal 3: Errors ───────────────────────────────────────────────
    pub http_errors_total: CounterVec,

    // ── Golden Signal 4: Saturation (DB pool) ────────────────────────────────
    pub db_pool_connections_active: Gauge,
    pub db_pool_connections_idle: Gauge,
    pub db_pool_connections_max: Gauge,

    // ── #10: Request size ─────────────────────────────────────────────────────
    pub http_request_size_bytes: HistogramVec,

    // ── #11-12: NATS queue ────────────────────────────────────────────────────
    pub nats_messages_published_total: CounterVec,
    pub nats_publish_errors_total: CounterVec,

    // ── #13: Worker jobs ──────────────────────────────────────────────────────
    pub worker_jobs_processed_total: CounterVec,

    // ── #14: Email ────────────────────────────────────────────────────────────
    pub email_sends_total: CounterVec,

    // ── #15: Push notifications ───────────────────────────────────────────────
    pub push_notification_sends_total: CounterVec,

    // ── #16: Redis ────────────────────────────────────────────────────────────
    pub redis_operations_total: CounterVec,

    registry: Registry,
}

impl Metrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        // Process collector provides cpu, memory, open-fds, etc.
        // Only available on Linux (procfs).
        #[cfg(target_os = "linux")]
        registry.register(Box::new(
            prometheus::process_collector::ProcessCollector::for_self(),
        ))?;

        let http_request_duration_seconds = register_histogram_vec_with_registry!(
            "http_request_duration_seconds",
            "HTTP request latency in seconds (golden signal: latency)",
            &["method", "route", "status"],
            // SLO-friendly buckets: 5 ms → 10 s
            vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
            registry
        )?;

        let http_requests_total = register_counter_vec_with_registry!(
            "http_requests_total",
            "Total HTTP requests received (golden signal: traffic)",
            &["method", "route", "status"],
            registry
        )?;

        let http_errors_total = register_counter_vec_with_registry!(
            "http_errors_total",
            "Total HTTP 4xx + 5xx responses (golden signal: errors)",
            &["method", "route", "status"],
            registry
        )?;

        let db_pool_connections_active = register_gauge_with_registry!(
            "db_pool_connections_active",
            "Active (in-use) database connections (golden signal: saturation)",
            registry
        )?;

        let db_pool_connections_idle = register_gauge_with_registry!(
            "db_pool_connections_idle",
            "Idle database connections currently in the pool",
            registry
        )?;

        let db_pool_connections_max = register_gauge_with_registry!(
            "db_pool_connections_max",
            "Configured maximum database pool size",
            registry
        )?;

        let http_request_size_bytes = register_histogram_vec_with_registry!(
            "http_request_size_bytes",
            "HTTP request body size in bytes (from Content-Length)",
            &["method", "route"],
            vec![64.0, 256.0, 1_024.0, 4_096.0, 16_384.0, 65_536.0, 262_144.0],
            registry
        )?;

        let nats_messages_published_total = register_counter_vec_with_registry!(
            "nats_messages_published_total",
            "Total NATS messages published successfully",
            &["job_type"],
            registry
        )?;

        let nats_publish_errors_total = register_counter_vec_with_registry!(
            "nats_publish_errors_total",
            "Total failed NATS publish attempts",
            &["job_type"],
            registry
        )?;

        let worker_jobs_processed_total = register_counter_vec_with_registry!(
            "worker_jobs_processed_total",
            "Total background jobs processed (labels: worker, job_type, status=ok|error)",
            &["worker", "job_type", "status"],
            registry
        )?;

        let email_sends_total = register_counter_vec_with_registry!(
            "email_sends_total",
            "Total SMTP email send attempts (label: status=ok|error)",
            &["status"],
            registry
        )?;

        let push_notification_sends_total = register_counter_vec_with_registry!(
            "push_notification_sends_total",
            "Total FCM push notification send attempts (labels: event, status=ok|error)",
            &["event", "status"],
            registry
        )?;

        let redis_operations_total = register_counter_vec_with_registry!(
            "redis_operations_total",
            "Total Redis operations (labels: operation, status=ok|error)",
            &["operation", "status"],
            registry
        )?;

        Ok(Self {
            http_request_duration_seconds,
            http_requests_total,
            http_errors_total,
            db_pool_connections_active,
            db_pool_connections_idle,
            db_pool_connections_max,
            http_request_size_bytes,
            nats_messages_published_total,
            nats_publish_errors_total,
            worker_jobs_processed_total,
            email_sends_total,
            push_notification_sends_total,
            redis_operations_total,
            registry,
        })
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> Vec<u8> {
        let encoder = TextEncoder::new();
        let families = self.registry.gather();
        let mut buf = Vec::new();
        encoder.encode(&families, &mut buf).unwrap_or(());
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics::new().expect("Metrics::new should never fail")
    }

    fn render_str(m: &Metrics) -> String {
        String::from_utf8(m.render()).expect("render output must be valid UTF-8")
    }

    // ── Construction ──────────────────────────────────────────────────────────

    #[test]
    fn new_succeeds() {
        metrics();
    }

    // ── render() output ───────────────────────────────────────────────────────

    #[test]
    fn render_contains_all_registered_metric_names() {
        let m = metrics();

        // Prometheus text format only emits a metric family after at least one
        // label combination has been observed.  Touch each one so they all appear.
        m.http_requests_total
            .with_label_values(&["GET", "/", "200"])
            .inc();
        m.http_errors_total
            .with_label_values(&["GET", "/", "500"])
            .inc();
        m.http_request_duration_seconds
            .with_label_values(&["GET", "/", "200"])
            .observe(0.01);
        m.db_pool_connections_active.set(1.0);
        m.db_pool_connections_idle.set(1.0);
        m.db_pool_connections_max.set(10.0);
        m.http_request_size_bytes
            .with_label_values(&["POST", "/"])
            .observe(128.0);
        m.nats_messages_published_total
            .with_label_values(&["job"])
            .inc();
        m.nats_publish_errors_total
            .with_label_values(&["job"])
            .inc();
        m.worker_jobs_processed_total
            .with_label_values(&["w", "j", "ok"])
            .inc();
        m.email_sends_total.with_label_values(&["ok"]).inc();
        m.push_notification_sends_total
            .with_label_values(&["event", "ok"])
            .inc();
        m.redis_operations_total
            .with_label_values(&["get", "ok"])
            .inc();

        let output = render_str(&m);

        let expected_names = [
            "http_request_duration_seconds",
            "http_requests_total",
            "http_errors_total",
            "db_pool_connections_active",
            "db_pool_connections_idle",
            "db_pool_connections_max",
            "http_request_size_bytes",
            "nats_messages_published_total",
            "nats_publish_errors_total",
            "worker_jobs_processed_total",
            "email_sends_total",
            "push_notification_sends_total",
            "redis_operations_total",
        ];

        for name in expected_names {
            assert!(
                output.contains(name),
                "render() output is missing metric '{name}'"
            );
        }
    }

    #[test]
    fn render_produces_valid_prometheus_text_format() {
        let m = metrics();
        m.http_requests_total
            .with_label_values(&["GET", "/api/v1/health", "200"])
            .inc();

        let output = render_str(&m);
        // Prometheus text format requires HELP and TYPE comment lines per family.
        assert!(
            output.contains("# HELP http_requests_total"),
            "missing HELP line"
        );
        assert!(
            output.contains("# TYPE http_requests_total counter"),
            "missing TYPE line"
        );
    }

    // ── Golden signal 2: Traffic counter ─────────────────────────────────────

    #[test]
    fn http_requests_total_increments() {
        let m = metrics();
        let labels = &["GET", "/api/v1/health", "200"];

        m.http_requests_total.with_label_values(labels).inc();
        m.http_requests_total.with_label_values(labels).inc();

        assert_eq!(m.http_requests_total.with_label_values(labels).get(), 2.0);
    }

    // ── Golden signal 3: Error counter ───────────────────────────────────────

    #[test]
    fn http_errors_total_increments_for_4xx() {
        let m = metrics();
        let labels = &["GET", "/api/v1/meetings/unknown", "404"];
        m.http_errors_total.with_label_values(labels).inc();
        assert_eq!(m.http_errors_total.with_label_values(labels).get(), 1.0);
    }

    #[test]
    fn http_errors_total_increments_for_5xx() {
        let m = metrics();
        let labels = &["POST", "/api/v1/meetings", "500"];
        m.http_errors_total.with_label_values(labels).inc();
        assert_eq!(m.http_errors_total.with_label_values(labels).get(), 1.0);
    }

    // ── Golden signal 1: Latency histogram ───────────────────────────────────

    #[test]
    fn http_request_duration_seconds_observe_appears_in_render() {
        let m = metrics();
        m.http_request_duration_seconds
            .with_label_values(&["GET", "/api/v1/health", "200"])
            .observe(0.042);

        let output = render_str(&m);
        // A histogram in Prometheus text format emits _count and _sum series.
        assert!(
            output.contains("http_request_duration_seconds_count"),
            "missing histogram count"
        );
        assert!(
            output.contains("http_request_duration_seconds_sum"),
            "missing histogram sum"
        );
    }

    // ── Golden signal 4: Saturation gauges ───────────────────────────────────

    #[test]
    fn db_pool_gauges_set_and_read_correctly() {
        let m = metrics();
        m.db_pool_connections_max.set(10.0);
        m.db_pool_connections_active.set(7.0);
        m.db_pool_connections_idle.set(3.0);

        assert_eq!(m.db_pool_connections_max.get(), 10.0);
        assert_eq!(m.db_pool_connections_active.get(), 7.0);
        assert_eq!(m.db_pool_connections_idle.get(), 3.0);
    }

    // ── Request size histogram ────────────────────────────────────────────────

    #[test]
    fn http_request_size_bytes_observe_appears_in_render() {
        let m = metrics();
        m.http_request_size_bytes
            .with_label_values(&["POST", "/api/v1/meetings"])
            .observe(512.0);

        let output = render_str(&m);
        assert!(output.contains("http_request_size_bytes_count"));
    }

    // ── NATS counters ─────────────────────────────────────────────────────────

    #[test]
    fn nats_publish_and_error_counters() {
        let m = metrics();

        m.nats_messages_published_total
            .with_label_values(&["schedule_meeting"])
            .inc_by(3.0);
        m.nats_publish_errors_total
            .with_label_values(&["schedule_meeting"])
            .inc();

        assert_eq!(
            m.nats_messages_published_total
                .with_label_values(&["schedule_meeting"])
                .get(),
            3.0,
            "published counter"
        );
        assert_eq!(
            m.nats_publish_errors_total
                .with_label_values(&["schedule_meeting"])
                .get(),
            1.0,
            "error counter"
        );
    }

    // ── Worker jobs counter ───────────────────────────────────────────────────

    #[test]
    fn worker_jobs_processed_tracks_ok_and_error_separately() {
        let m = metrics();

        m.worker_jobs_processed_total
            .with_label_values(&["notification_worker", "send_notification", "ok"])
            .inc();
        m.worker_jobs_processed_total
            .with_label_values(&["notification_worker", "send_notification", "error"])
            .inc();

        assert_eq!(
            m.worker_jobs_processed_total
                .with_label_values(&["notification_worker", "send_notification", "ok"])
                .get(),
            1.0
        );
        assert_eq!(
            m.worker_jobs_processed_total
                .with_label_values(&["notification_worker", "send_notification", "error"])
                .get(),
            1.0
        );
    }

    // ── Email & push-notification counters ────────────────────────────────────

    #[test]
    fn email_sends_total_tracks_status() {
        let m = metrics();
        m.email_sends_total.with_label_values(&["ok"]).inc();
        m.email_sends_total.with_label_values(&["error"]).inc();

        assert_eq!(m.email_sends_total.with_label_values(&["ok"]).get(), 1.0);
        assert_eq!(m.email_sends_total.with_label_values(&["error"]).get(), 1.0);
    }

    #[test]
    fn push_notification_sends_total_tracks_event_and_status() {
        let m = metrics();
        m.push_notification_sends_total
            .with_label_values(&["meeting_reminder", "ok"])
            .inc();

        assert_eq!(
            m.push_notification_sends_total
                .with_label_values(&["meeting_reminder", "ok"])
                .get(),
            1.0
        );
    }

    // ── Redis counter ─────────────────────────────────────────────────────────

    #[test]
    fn redis_operations_total_tracks_operation_and_status() {
        let m = metrics();
        m.redis_operations_total
            .with_label_values(&["get", "ok"])
            .inc_by(5.0);
        m.redis_operations_total
            .with_label_values(&["set", "error"])
            .inc();

        assert_eq!(
            m.redis_operations_total
                .with_label_values(&["get", "ok"])
                .get(),
            5.0
        );
        assert_eq!(
            m.redis_operations_total
                .with_label_values(&["set", "error"])
                .get(),
            1.0
        );
    }

    // ── Independent registries do not interfere ───────────────────────────────

    #[test]
    fn two_metrics_instances_have_independent_registries() {
        let m1 = metrics();
        let m2 = metrics();

        m1.http_requests_total
            .with_label_values(&["GET", "/", "200"])
            .inc_by(5.0);

        // m2 was never incremented; its value must still be 0.
        assert_eq!(
            m2.http_requests_total
                .with_label_values(&["GET", "/", "200"])
                .get(),
            0.0,
            "Metrics instances must not share state"
        );
    }
}
