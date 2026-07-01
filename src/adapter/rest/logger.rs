use crate::logging::{self, Level};
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use std::{
    future::{ready, Future, Ready},
    pin::Pin,
    time::Instant,
};

/// Actix-web middleware that logs every HTTP request via `crate::logging`.
///
/// The emitted log level depends on the response status:
///   5xx  → ERROR
///   4xx  → WARN
///   /auth or /admin paths → AUDIT
///   everything else       → INFO
///
/// At DEBUG level an additional entry is emitted with User-Agent and peer address.
pub struct LoggixLogger;

impl<S, B> Transform<S, ServiceRequest> for LoggixLogger
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = LoggixLoggerMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(LoggixLoggerMiddleware { service }))
    }
}

pub struct LoggixLoggerMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for LoggixLoggerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
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
        let peer = req
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "-".to_string());
        let user_agent = req
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();

        let fut = self.service.call(req);

        Box::pin(async move {
            let start = Instant::now();
            let res = fut.await?;
            let elapsed_ms = start.elapsed().as_millis().to_string();
            let status = res.status().as_u16();
            let status_s = status.to_string();

            let log_level = resolve_level(status, &path);

            if logging::is_enabled(log_level) {
                logging::emit_for_middleware(
                    log_level,
                    &[
                        ("method",     &method),
                        ("path",       &path),
                        ("status",     &status_s),
                        ("elapsed_ms", &elapsed_ms),
                    ],
                    "request",
                );
            }

            // DEBUG: emit a second entry with peer + User-Agent detail
            if logging::is_enabled(Level::Debug) {
                logging::emit_for_middleware(
                    Level::Debug,
                    &[
                        ("method",     &method),
                        ("path",       &path),
                        ("peer",       &peer),
                        ("user_agent", &user_agent),
                    ],
                    "request detail",
                );
            }

            Ok(res)
        })
    }
}

/// Map HTTP status code (and path semantics) to a log level.
fn resolve_level(status: u16, path: &str) -> Level {
    if status >= 500 {
        Level::Error
    } else if status >= 400 {
        Level::Warn
    } else if is_audit_path(path) {
        Level::Audit
    } else {
        Level::Info
    }
}

/// Paths that carry security significance — logged at AUDIT level on success.
fn is_audit_path(path: &str) -> bool {
    path.contains("/auth") || path.contains("/admin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test as atest, web, App, HttpResponse};

    // ── resolve_level ─────────────────────────────────────────────────────────

    #[test]
    fn test_resolve_level_5xx_is_error() {
        assert_eq!(resolve_level(503, "/api/v1/meetings"), Level::Error);
    }

    #[test]
    fn test_resolve_level_500_is_error() {
        assert_eq!(resolve_level(500, "/api/v1/meetings"), Level::Error);
    }

    #[test]
    fn test_resolve_level_4xx_is_warn() {
        assert_eq!(resolve_level(403, "/api/v1/meetings"), Level::Warn);
    }

    #[test]
    fn test_resolve_level_400_is_warn() {
        assert_eq!(resolve_level(400, "/api/v1/meetings"), Level::Warn);
    }

    #[test]
    fn test_resolve_level_auth_path_is_audit() {
        assert_eq!(resolve_level(200, "/api/v1/auth/login"), Level::Audit);
    }

    #[test]
    fn test_resolve_level_admin_path_is_audit() {
        assert_eq!(resolve_level(200, "/api/v1/admin/something"), Level::Audit);
    }

    #[test]
    fn test_resolve_level_normal_200_is_info() {
        assert_eq!(resolve_level(200, "/api/v1/meetings"), Level::Info);
    }

    // ── is_audit_path ─────────────────────────────────────────────────────────

    #[test]
    fn test_is_audit_path_auth() {
        assert!(is_audit_path("/auth/google/callback"));
    }

    #[test]
    fn test_is_audit_path_admin() {
        assert!(is_audit_path("/admin/log-level"));
    }

    #[test]
    fn test_is_audit_path_regular() {
        assert!(!is_audit_path("/meetings"));
    }

    // ── Middleware integration ─────────────────────────────────────────────────

    #[actix_web::test]
    async fn test_middleware_passes_200_through() {
        let app = atest::init_service(
            App::new()
                .wrap(LoggixLogger)
                .route("/ping", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = atest::TestRequest::get().uri("/ping").to_request();
        let res = atest::call_service(&app, req).await;
        assert_eq!(res.status().as_u16(), 200);
    }

    #[actix_web::test]
    async fn test_middleware_passes_404_through() {
        let app = atest::init_service(
            App::new()
                .wrap(LoggixLogger)
                .route("/ping", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = atest::TestRequest::get().uri("/nonexistent").to_request();
        let res = atest::call_service(&app, req).await;
        assert_eq!(res.status().as_u16(), 404);
    }

    #[actix_web::test]
    async fn test_middleware_passes_500_through() {
        let app = atest::init_service(
            App::new()
                .wrap(LoggixLogger)
                .route(
                    "/boom",
                    web::get()
                        .to(|| async { HttpResponse::InternalServerError().finish() }),
                ),
        )
        .await;

        let req = atest::TestRequest::get().uri("/boom").to_request();
        let res = atest::call_service(&app, req).await;
        assert_eq!(res.status().as_u16(), 500);
    }
}
