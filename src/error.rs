use actix_web::{HttpResponse, ResponseError};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Email error: {0}")]
    Email(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl AppError {
    fn error_code(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "NOT_FOUND",
            AppError::Unauthorized(_) => "UNAUTHORIZED",
            AppError::Forbidden(_) => "FORBIDDEN",
            AppError::BadRequest(_) => "BAD_REQUEST",
            AppError::Validation(_) => "VALIDATION_ERROR",
            AppError::Conflict(_) => "CONFLICT",
            AppError::Database(_) => "DATABASE_ERROR",
            AppError::Http(_) => "HTTP_ERROR",
            AppError::Email(_) => "EMAIL_ERROR",
            AppError::Queue(_) => "QUEUE_ERROR",
            AppError::Config(_) => "CONFIG_ERROR",
            AppError::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

impl ResponseError for AppError {
    fn error_response(&self) -> HttpResponse {
        use actix_web::http::StatusCode;

        let status = match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) | AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        // Avoid leaking internal details in production
        let message = match self {
            AppError::Database(_) | AppError::Internal(_) => {
                crate::logging::error_with(&[("error", &self.to_string())], "internal error");
                "An internal error occurred".to_string()
            }
            other => other.to_string(),
        };

        HttpResponse::build(status).json(json!({
            "error": {
                "code": self.error_code(),
                "message": message
            }
        }))
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::ResponseError;

    #[test]
    fn not_found_is_404() {
        let err = AppError::NotFound("thing".into());
        assert_eq!(err.error_response().status(), 404);
    }

    #[test]
    fn unauthorized_is_401() {
        let err = AppError::Unauthorized("bad token".into());
        assert_eq!(err.error_response().status(), 401);
    }

    #[test]
    fn forbidden_is_403() {
        let err = AppError::Forbidden("no access".into());
        assert_eq!(err.error_response().status(), 403);
    }

    #[test]
    fn bad_request_is_400() {
        let err = AppError::BadRequest("bad input".into());
        assert_eq!(err.error_response().status(), 400);
    }

    #[test]
    fn validation_is_400() {
        let err = AppError::Validation("invalid field".into());
        assert_eq!(err.error_response().status(), 400);
    }

    #[test]
    fn conflict_is_409() {
        let err = AppError::Conflict("already exists".into());
        assert_eq!(err.error_response().status(), 409);
    }

    #[test]
    fn internal_is_500() {
        let err = AppError::Internal("something went wrong".into());
        assert_eq!(err.error_response().status(), 500);
    }

    #[test]
    fn email_error_is_500() {
        let err = AppError::Email("smtp failure".into());
        assert_eq!(err.error_response().status(), 500);
    }

    #[test]
    fn queue_error_is_500() {
        let err = AppError::Queue("queue full".into());
        assert_eq!(err.error_response().status(), 500);
    }

    #[test]
    fn config_error_is_500() {
        let err = AppError::Config("missing env var".into());
        assert_eq!(err.error_response().status(), 500);
    }

    #[test]
    fn error_codes() {
        assert_eq!(AppError::NotFound("x".into()).error_code(), "NOT_FOUND");
        assert_eq!(AppError::Unauthorized("x".into()).error_code(), "UNAUTHORIZED");
        assert_eq!(AppError::Forbidden("x".into()).error_code(), "FORBIDDEN");
        assert_eq!(AppError::BadRequest("x".into()).error_code(), "BAD_REQUEST");
        assert_eq!(AppError::Validation("x".into()).error_code(), "VALIDATION_ERROR");
        assert_eq!(AppError::Conflict("x".into()).error_code(), "CONFLICT");
        assert_eq!(AppError::Email("x".into()).error_code(), "EMAIL_ERROR");
        assert_eq!(AppError::Queue("x".into()).error_code(), "QUEUE_ERROR");
        assert_eq!(AppError::Config("x".into()).error_code(), "CONFIG_ERROR");
        assert_eq!(AppError::Internal("x".into()).error_code(), "INTERNAL_ERROR");
    }

    #[test]
    fn from_anyhow_gives_internal() {
        let err = AppError::from(anyhow::anyhow!("oops"));
        assert!(matches!(err, AppError::Internal(_)));
    }
}
