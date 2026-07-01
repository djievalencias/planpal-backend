/// OTLP distributed-tracing initialisation.
///
/// Initialises a global OpenTelemetry tracer that exports spans via gRPC to
/// Grafana Tempo (or any OTLP-compatible collector).  A `tracing-opentelemetry`
/// layer bridges Rust's `tracing` crate to the OTel SDK so every
/// `tracing::info_span!` / `#[tracing::instrument]` call generates a proper span.
///
/// Configuration (via `AppConfig::otlp`):
///
///   APP__OTLP__ENDPOINT      = http://tempo:4317    (gRPC; leave empty to disable)
///   APP__OTLP__SAMPLING_RATE = 0.1                  (0.0 – 1.0)
///   APP__OTLP__SERVICE_NAME  = planpal-server
use crate::config::OtlpConfig;
use opentelemetry::{trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    runtime::Tokio,
    trace::{Config, Sampler, TracerProvider},
    Resource,
};

/// RAII guard that shuts down the global tracer on drop, flushing pending spans.
/// Hold this value for the lifetime of the process.
pub struct TracingGuard {
    provider: TracerProvider,
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Err(e) = self.provider.shutdown() {
            eprintln!("[telemetry] tracer shutdown error: {e}");
        }
    }
}

/// Initialise the global OTLP tracer and wire up the `tracing` → OTel bridge.
///
/// Returns `None` when `cfg.endpoint` is empty (tracing disabled).
/// Returns `Some(TracingGuard)` on success — must be kept alive until process exit.
pub fn init(cfg: &OtlpConfig) -> anyhow::Result<Option<TracingGuard>> {
    if cfg.endpoint.is_empty() {
        return Ok(None);
    }

    let exporter = match cfg.transport.as_str() {
        "http" => opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(&cfg.endpoint)
            .build_span_exporter()?,
        _ => opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(&cfg.endpoint)
            .build_span_exporter()?,
    };

    let sampler = match cfg.sampling_rate {
        r if r >= 1.0 => Sampler::AlwaysOn,
        r if r <= 0.0 => Sampler::AlwaysOff,
        r => Sampler::TraceIdRatioBased(r),
    };

    let resource = Resource::new(vec![
        KeyValue::new("service.name", cfg.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);

    let trace_config = Config::default()
        .with_sampler(sampler)
        .with_resource(resource);

    let provider = TracerProvider::builder()
        .with_config(trace_config)
        .with_batch_exporter(exporter, Tokio)
        .build();

    // Register as the global provider.
    opentelemetry::global::set_tracer_provider(provider.clone());

    // Bridge `tracing` spans into the OTel SDK.
    // Must call .with_tracer() explicitly — layer() alone defaults to NoopTracer.
    let tracer = provider.tracer(cfg.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(otel_layer)
        .try_init()
        .ok(); // silently ignore "already initialised" in tests

    Ok(Some(TracingGuard { provider }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::testing::trace::InMemorySpanExporter;
    use opentelemetry_sdk::trace::TracerProvider;
    use tracing_subscriber::prelude::*;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a local `TracerProvider` backed by an `InMemorySpanExporter`.
    /// Does NOT touch any global state.
    fn local_provider() -> (TracerProvider, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }

    // ── init() return value ───────────────────────────────────────────────────

    #[test]
    fn init_returns_none_for_empty_endpoint() {
        let cfg = OtlpConfig {
            endpoint: String::new(),
            sampling_rate: 0.1,
            service_name: "test".into(),
            transport: "grpc".into(),
        };
        let guard = init(&cfg).expect("init must not error for empty endpoint");
        assert!(guard.is_none(), "empty endpoint must yield None");
    }

    // ── Span recording via in-memory exporter ─────────────────────────────────

    #[test]
    fn span_is_exported_with_correct_name() {
        let (provider, exporter) = local_provider();
        let tracer = provider.tracer("test");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(otel_layer),
            || {
                let _span = tracing::info_span!("my.operation").entered();
                // span exits here
            },
        );

        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1, "exactly one span should be exported");
        assert_eq!(spans[0].name, "my.operation");
    }

    #[test]
    fn nested_spans_produce_parent_child_relationship() {
        let (provider, exporter) = local_provider();
        let tracer = provider.tracer("test");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(otel_layer),
            || {
                let _parent = tracing::info_span!("parent.op").entered();
                let _child = tracing::info_span!("child.op").entered();
                // child exits, then parent exits
            },
        );

        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 2, "both parent and child spans exported");

        // The child span's parent_span_id must point to the parent span.
        let parent = spans.iter().find(|s| s.name == "parent.op").unwrap();
        let child = spans.iter().find(|s| s.name == "child.op").unwrap();
        assert_eq!(
            child.parent_span_id,
            parent.span_context.span_id(),
            "child must reference parent span id"
        );
    }

    #[test]
    fn always_off_sampler_drops_all_spans() {
        use opentelemetry_sdk::trace::{Config, Sampler, TracerProvider};

        let exporter = InMemorySpanExporter::default();
        let provider = TracerProvider::builder()
            .with_config(Config::default().with_sampler(Sampler::AlwaysOff))
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(otel_layer),
            || {
                let _span = tracing::info_span!("should.be.dropped").entered();
            },
        );

        let spans = exporter.get_finished_spans().unwrap();
        assert!(
            spans.is_empty(),
            "AlwaysOff sampler must not export any spans"
        );
    }

    #[test]
    fn always_on_sampler_records_all_spans() {
        use opentelemetry_sdk::trace::{Config, Sampler, TracerProvider};

        let exporter = InMemorySpanExporter::default();
        let provider = TracerProvider::builder()
            .with_config(Config::default().with_sampler(Sampler::AlwaysOn))
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(otel_layer),
            || {
                let _a = tracing::info_span!("op.a").entered();
                let _b = tracing::info_span!("op.b").entered();
            },
        );

        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(
            spans.len(),
            2,
            "AlwaysOn sampler must export every span"
        );
    }

    // ── TracingGuard drop ─────────────────────────────────────────────────────

    #[test]
    fn tracing_guard_drop_does_not_panic() {
        // Build a provider backed by our in-memory exporter (no real network).
        // Dropping the guard calls provider.shutdown() — must not panic.
        let (provider, _exporter) = local_provider();
        let guard = TracingGuard { provider };
        drop(guard); // panics here → test fails
    }
}
