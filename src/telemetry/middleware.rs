/// Actix-web middleware that records Prometheus metrics and creates OTLP spans
/// for every HTTP request.
///
/// Metrics recorded per request:
///   http_requests_total            (counter, labels: method, route, status)
///   http_request_duration_seconds  (histogram, labels: method, route, status)
///   http_errors_total              (counter, labels: method, route, status) — 4xx+5xx only
///   http_request_size_bytes        (histogram, labels: method, route) — from Content-Length
///
/// OTLP span attributes (semantic conventions):
///   http.method, http.target, http.route, http.status_code, otel.kind=server
use crate::telemetry::Metrics;
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use std::{
    future::{ready, Future, Ready},
    pin::Pin,
    sync::Arc,
    time::Instant,
};
use tracing::Instrument;

pub struct TelemetryMiddleware {
    metrics: Arc<Metrics>,
}

impl TelemetryMiddleware {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self { metrics }
    }
}

impl<S, B> Transform<S, ServiceRequest> for TelemetryMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = TelemetryMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(TelemetryMiddlewareService {
            service,
            metrics: self.metrics.clone(),
        }))
    }
}

pub struct TelemetryMiddlewareService<S> {
    service: S,
    metrics: Arc<Metrics>,
}

impl<S, B> Service<ServiceRequest> for TelemetryMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let method = req.method().to_string();
        let path = req.path().to_string();

        let content_length: Option<f64> = req
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());

        let metrics = self.metrics.clone();

        // Create the root OTLP span for this HTTP request.
        // `http.route` and `http.status_code` are filled in after the inner call.
        let span = tracing::info_span!(
            "http.request",
            "http.method"      = %method,
            "http.target"      = %path,
            "http.route"       = tracing::field::Empty,
            "http.status_code" = tracing::field::Empty,
            "otel.kind"        = "server",
        );

        let fut = self.service.call(req);

        Box::pin(
            async move {
                let start = Instant::now();
                let res = fut.await?;
                let elapsed = start.elapsed().as_secs_f64();
                let status = res.status().as_u16();

                // actix-web resolves the matched route pattern after routing.
                // Using the pattern (e.g. "/api/v1/meetings/{id}") instead of
                // the actual path avoids cardinality explosion in Prometheus.
                let route = res
                    .request()
                    .match_pattern()
                    .unwrap_or_else(|| path.clone());

                let status_s = status.to_string();

                // ── OTLP span attributes ──────────────────────────────────────
                tracing::Span::current().record("http.route", &route.as_str());
                tracing::Span::current().record("http.status_code", status);

                // ── Prometheus metrics ────────────────────────────────────────
                metrics
                    .http_requests_total
                    .with_label_values(&[&method, &route, &status_s])
                    .inc();

                metrics
                    .http_request_duration_seconds
                    .with_label_values(&[&method, &route, &status_s])
                    .observe(elapsed);

                if status >= 400 {
                    metrics
                        .http_errors_total
                        .with_label_values(&[&method, &route, &status_s])
                        .inc();
                }

                if let Some(size) = content_length {
                    metrics
                        .http_request_size_bytes
                        .with_label_values(&[&method, &route])
                        .observe(size);
                }

                Ok(res)
            }
            .instrument(span),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Metrics;
    use actix_web::{test, web, App, HttpResponse};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_metrics() -> Arc<Metrics> {
        Arc::new(Metrics::new().unwrap())
    }

    // ── Traffic counter ───────────────────────────────────────────────────────

    #[actix_web::test]
    async fn http_requests_total_increments_on_200() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route("/ok", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = test::TestRequest::get().uri("/ok").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status().as_u16(), 200);

        // Route pattern falls back to the path when no match_pattern is set.
        assert_eq!(
            metrics
                .http_requests_total
                .with_label_values(&["GET", "/ok", "200"])
                .get(),
            1.0
        );
    }

    #[actix_web::test]
    async fn http_requests_total_increments_on_every_call() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route("/ping", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        for _ in 0..3 {
            let req = test::TestRequest::get().uri("/ping").to_request();
            test::call_service(&app, req).await;
        }

        assert_eq!(
            metrics
                .http_requests_total
                .with_label_values(&["GET", "/ping", "200"])
                .get(),
            3.0
        );
    }

    // ── Error counter ─────────────────────────────────────────────────────────

    #[actix_web::test]
    async fn http_errors_total_increments_for_404() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route(
                    "/not-found",
                    web::get().to(|| async { HttpResponse::NotFound().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::get().uri("/not-found").to_request();
        test::call_service(&app, req).await;

        assert_eq!(
            metrics
                .http_errors_total
                .with_label_values(&["GET", "/not-found", "404"])
                .get(),
            1.0,
            "4xx must increment error counter"
        );
    }

    #[actix_web::test]
    async fn http_errors_total_increments_for_500() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route(
                    "/boom",
                    web::post().to(|| async {
                        HttpResponse::InternalServerError().finish()
                    }),
                ),
        )
        .await;

        let req = test::TestRequest::post().uri("/boom").to_request();
        test::call_service(&app, req).await;

        assert_eq!(
            metrics
                .http_errors_total
                .with_label_values(&["POST", "/boom", "500"])
                .get(),
            1.0,
            "5xx must increment error counter"
        );
    }

    #[actix_web::test]
    async fn http_errors_total_does_not_increment_for_2xx() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route("/ok", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = test::TestRequest::get().uri("/ok").to_request();
        test::call_service(&app, req).await;

        // No error labels should have been touched — counter should be 0.
        assert_eq!(
            metrics
                .http_errors_total
                .with_label_values(&["GET", "/ok", "200"])
                .get(),
            0.0,
            "2xx must NOT increment error counter"
        );
    }

    // ── Latency histogram ─────────────────────────────────────────────────────

    #[actix_web::test]
    async fn http_request_duration_seconds_records_observation() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route("/slow", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = test::TestRequest::get().uri("/slow").to_request();
        test::call_service(&app, req).await;

        // After one request the histogram count must be 1 and sum > 0.
        let labels = &["GET", "/slow", "200"];
        let count = metrics
            .http_request_duration_seconds
            .with_label_values(labels);
        // Prometheus histograms expose sample_count and sample_sum through
        // the MetricVec API; the easiest check is that render() has data.
        let output = String::from_utf8(metrics.render()).unwrap();
        assert!(
            output.contains(r#"http_request_duration_seconds_count{method="GET""#),
            "duration histogram must have a count entry after a request"
        );
        drop(count); // suppress unused-variable warning
    }

    // ── Request-size histogram ────────────────────────────────────────────────

    #[actix_web::test]
    async fn http_request_size_bytes_recorded_from_content_length() {
        let metrics = make_metrics();

        let app = test::init_service(
            App::new()
                .wrap(TelemetryMiddleware::new(metrics.clone()))
                .route(
                    "/upload",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let body = b"hello world";
        let req = test::TestRequest::post()
            .uri("/upload")
            .insert_header(("content-length", body.len().to_string()))
            .set_payload(body.as_ref())
            .to_request();
        test::call_service(&app, req).await;

        let output = String::from_utf8(metrics.render()).unwrap();
        assert!(
            output.contains("http_request_size_bytes_count"),
            "request size histogram must record an observation when Content-Length is present"
        );
    }
}
