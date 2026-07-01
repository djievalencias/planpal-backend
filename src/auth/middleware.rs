use crate::{
    auth::jwt::decode_token,
    error::AppError,
    model::user::{User, UserRole},
    repository::user_repo,
    AppState,
};
use actix_web::{dev::Payload, web, FromRequest, HttpRequest};
use std::{future::Future, pin::Pin};
use uuid::Uuid;

/// Extractor that validates the Bearer JWT and resolves the calling user.
/// Succeeds for any authenticated user regardless of role.
pub struct AuthenticatedUser(pub User);

impl FromRequest for AuthenticatedUser {
    type Error = AppError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let req = req.clone();
        Box::pin(async move {
            let user = resolve_user(&req).await?;
            Ok(AuthenticatedUser(user))
        })
    }
}

/// Extractor that validates the Bearer JWT **and** requires `role = admin`.
/// Returns `403 Forbidden` for authenticated non-admin users.
pub struct AdminUser(pub User);

impl FromRequest for AdminUser {
    type Error = AppError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let req = req.clone();
        Box::pin(async move {
            let user = resolve_user(&req).await?;
            if user.role != UserRole::Admin {
                return Err(AppError::Forbidden(
                    "admin role required".into(),
                ));
            }
            Ok(AdminUser(user))
        })
    }
}

/// Shared logic: extract Bearer token → decode JWT → load User from DB.
async fn resolve_user(req: &HttpRequest) -> Result<User, AppError> {
    let state = req
        .app_data::<web::Data<AppState>>()
        .ok_or_else(|| AppError::Internal("missing app state".into()))?;

    let token = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            AppError::Unauthorized("missing or malformed Authorization header".into())
        })?;

    let claims = decode_token(token, &state.config.jwt.secret)?;

    let user_id: Uuid = claims
        .sub
        .parse()
        .map_err(|_| AppError::Unauthorized("invalid subject in token".into()))?;

    let user = user_repo::find_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    Ok(user)
}
