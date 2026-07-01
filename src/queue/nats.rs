use crate::{config::NatsConfig, error::AppError, queue::Job, telemetry::Metrics};
use async_nats::Client;
use bytes::Bytes;
use std::sync::Arc;
use tracing::Instrument;

/// Connect to the NATS server.
pub async fn connect(cfg: &NatsConfig) -> Result<Client, AppError> {
    async_nats::connect(&cfg.url)
        .await
        .map_err(|e| AppError::Queue(format!("NATS connect failed: {e}")))
}

/// Serialise a `Job`, publish it to the appropriate subject, and record metrics.
///
/// Creates an OTLP producer span and increments `nats_messages_published_total`
/// (or `nats_publish_errors_total` on failure).
pub async fn publish(
    nats: &Client,
    cfg: &NatsConfig,
    job: &Job,
    metrics: &Arc<Metrics>,
) -> Result<(), AppError> {
    let job_type = job.name();
    let subject = job.subject(&cfg.subject_prefix);

    let span = tracing::info_span!(
        "nats.publish",
        "messaging.system"      = "nats",
        "messaging.destination" = %subject,
        "messaging.operation"   = "publish",
        "messaging.message_type"= job_type,
        "otel.kind"             = "producer",
    );

    let result = async {
        let payload = serde_json::to_vec(job)
            .map_err(|e| AppError::Internal(format!("job serialise failed: {e}")))?;
        nats.publish(subject, Bytes::from(payload))
            .await
            .map_err(|e| AppError::Queue(format!("NATS publish failed: {e}")))
    }
    .instrument(span)
    .await;

    if result.is_ok() {
        metrics
            .nats_messages_published_total
            .with_label_values(&[job_type])
            .inc();
    } else {
        metrics
            .nats_publish_errors_total
            .with_label_values(&[job_type])
            .inc();
    }

    result
}
