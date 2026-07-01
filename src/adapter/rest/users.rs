use crate::{
    auth::middleware::AuthenticatedUser,
    error::AppError,
    model::user::UserProfile,
    repository::{event_repo, meeting_repo, user_repo},
    AppState,
};
use actix_web::{get, patch, web, HttpResponse};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_me)
        .service(update_me)
        .service(search_users)
        .service(check_availability);
}

// ── GET /users/me ─────────────────────────────────────────────────────────────

#[get("/users/me")]
async fn get_me(auth: AuthenticatedUser) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(UserProfile::from(auth.0)))
}

// ── PATCH /users/me ───────────────────────────────────────────────────────────

#[derive(Deserialize, Validate)]
struct UpdateMeRequest {
    #[validate(length(min = 1, max = 100))]
    display_name: Option<String>,
    fcm_token: Option<String>,
    #[validate(length(max = 100))]
    timezone: Option<String>,
    #[validate(length(max = 100))]
    department: Option<String>,
    #[validate(length(max = 100))]
    job_title: Option<String>,
    work_start: Option<String>,
    work_end: Option<String>,
    #[validate(length(max = 100))]
    manager_name: Option<String>,
    public_holidays: Option<Vec<String>>,
}

fn is_valid_time(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' { return false; }
    let h: u8 = s[..2].parse().unwrap_or(99);
    let m: u8 = s[3..].parse().unwrap_or(99);
    h < 24 && m < 60
}

#[patch("/users/me")]
async fn update_me(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    body: web::Json<UpdateMeRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate().map_err(|e| AppError::Validation(e.to_string()))?;

    if let Some(ref t) = body.work_start {
        if !t.is_empty() && !is_valid_time(t) {
            return Err(AppError::Validation("work_start must be HH:MM".into()));
        }
    }
    if let Some(ref t) = body.work_end {
        if !t.is_empty() && !is_valid_time(t) {
            return Err(AppError::Validation("work_end must be HH:MM".into()));
        }
    }

    if let Some(ref token) = body.fcm_token {
        user_repo::update_fcm_token(&state.db, auth.0.id, token).await?;
    }

    // None = field absent (keep); Some("") = Auto (clear to NULL); Some(tz) = set tz
    let timezone_param = body.timezone.as_deref();

    user_repo::update_profile(
        &state.db,
        auth.0.id,
        body.display_name.as_deref(),
        timezone_param,
        body.department.as_deref(),
        body.job_title.as_deref(),
        body.work_start.as_deref(),
        body.work_end.as_deref(),
        body.manager_name.as_deref(),
        body.public_holidays.clone(),
    )
    .await?;

    let updated = user_repo::find_by_id(&state.db, auth.0.id)
        .await?
        .ok_or_else(|| AppError::Internal("user disappeared after update".into()))?;

    Ok(HttpResponse::Ok().json(UserProfile::from(updated)))
}

// ── GET /users/search?q=... ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

/// Slim user representation returned by search — safe to expose to any authenticated user.
#[derive(Serialize)]
struct UserSummary {
    id: Uuid,
    display_name: String,
    email: String,
    job_title: Option<String>,
    department: Option<String>,
}

#[get("/users/search")]
async fn search_users(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    query: web::Query<SearchQuery>,
) -> Result<HttpResponse, AppError> {
    if query.q.trim().len() < 2 {
        return Ok(HttpResponse::Ok().json(Vec::<UserSummary>::new()));
    }
    let users = user_repo::search(&state.db, query.q.trim(), 10, auth.0.id).await?;
    let results: Vec<UserSummary> = users
        .into_iter()
        .map(|u| UserSummary {
            id: u.id,
            display_name: u.display_name,
            email: u.email,
            job_title: u.job_title,
            department: u.department,
        })
        .collect();
    Ok(HttpResponse::Ok().json(results))
}

// ── GET /users/availability?ids=uuid1,uuid2&from=ISO&until=ISO ────────────────

#[derive(Deserialize)]
struct AvailabilityQuery {
    /// Comma-separated UUID list
    ids: String,
    from: String,
    until: String,
}

#[derive(Serialize)]
struct ConflictEvent {
    title: String,
    start_at: String,
    end_at: String,
    /// "calendar" = external synced event, "planpal" = confirmed PlanPal meeting
    source: String,
}

/// Non-blocking policy warnings (public holiday, outside working hours).
#[derive(Serialize)]
struct AvailabilityWarning {
    /// "public_holiday" | "outside_working_hours"
    kind: String,
    message: String,
}

#[derive(Serialize)]
struct UserAvailability {
    user_id: Uuid,
    busy: bool,
    events: Vec<ConflictEvent>,
    warnings: Vec<AvailabilityWarning>,
}

#[get("/users/availability")]
async fn check_availability(
    state: web::Data<AppState>,
    _auth: AuthenticatedUser,
    query: web::Query<AvailabilityQuery>,
) -> Result<HttpResponse, AppError> {
    let from = query.from.parse::<DateTime<chrono::Utc>>()
        .map_err(|_| AppError::BadRequest("invalid 'from' datetime".into()))?;
    let until = query.until.parse::<DateTime<chrono::Utc>>()
        .map_err(|_| AppError::BadRequest("invalid 'until' datetime".into()))?;

    let ids: Vec<Uuid> = query.ids.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse::<Uuid>())
        .collect::<Result<_, _>>()
        .map_err(|_| AppError::BadRequest("invalid UUID in ids".into()))?;

    let mut results = Vec::with_capacity(ids.len());
    for user_id in ids {
        // ── 1. Calendar / PlanPal meeting conflicts ───────────────────────────
        let cal_events = event_repo::conflict_events_for_user(&state.db, user_id, from, until)
            .await
            .unwrap_or_default();
        let planpal_meetings = meeting_repo::confirmed_meetings_in_window(&state.db, user_id, from, until)
            .await
            .unwrap_or_default();

        let mut all_events: Vec<ConflictEvent> = cal_events
            .into_iter()
            .map(|(title, start_at, end_at)| ConflictEvent {
                title,
                start_at: start_at.to_rfc3339(),
                end_at: end_at.to_rfc3339(),
                source: "calendar".to_string(),
            })
            .collect();
        all_events.extend(planpal_meetings.into_iter().map(|(title, start_at, end_at)| ConflictEvent {
            title,
            start_at: start_at.to_rfc3339(),
            end_at: end_at.to_rfc3339(),
            source: "planpal".to_string(),
        }));
        all_events.sort_by(|a, b| a.start_at.cmp(&b.start_at));

        // ── 2. Policy warnings (best-effort; never fail the whole request) ────
        let mut warnings: Vec<AvailabilityWarning> = Vec::new();

        if let Ok(Some(user)) = user_repo::find_by_id(&state.db, user_id).await {
            // ── 2a. Public holiday check ──────────────────────────────────────
            if !user.public_holidays.is_empty() {
                if let Some(ref tz) = user.timezone {
                    // Convert the meeting's start to the user's local date and look
                    // it up in holiday_configs for any of their subscribed countries.
                    let holiday: Option<(String, String)> = sqlx::query_as(
                        "SELECT name, country
                         FROM holiday_configs
                         WHERE country = ANY($1::TEXT[])
                           AND date = ($2 AT TIME ZONE $3)::DATE
                         LIMIT 1",
                    )
                    .bind(&user.public_holidays)
                    .bind(from)
                    .bind(tz.as_str())
                    .fetch_optional(&state.db)
                    .await
                    .unwrap_or(None);

                    if let Some((name, country)) = holiday {
                        warnings.push(AvailabilityWarning {
                            kind: "public_holiday".to_string(),
                            message: format!("{name} is a public holiday in {country}"),
                        });
                    }
                }
            }

            // ── 2b. Outside working-hours check ──────────────────────────────
            if let (Some(ws), Some(we), Some(ref tz)) =
                (&user.work_start, &user.work_end, &user.timezone)
            {
                // Get meeting start/end as HH:MM strings in the user's local timezone.
                let times: Option<(String, String)> = sqlx::query_as(
                    "SELECT to_char($1 AT TIME ZONE $3, 'HH24:MI'),
                            to_char($2 AT TIME ZONE $3, 'HH24:MI')",
                )
                .bind(from)
                .bind(until)
                .bind(tz.as_str())
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);

                if let Some((local_start, local_end)) = times {
                    let before_work = local_start.as_str() < ws.as_str();
                    let after_work  = local_end.as_str()   > we.as_str();
                    if before_work || after_work {
                        warnings.push(AvailabilityWarning {
                            kind: "outside_working_hours".to_string(),
                            message: format!(
                                "Meeting time ({local_start}–{local_end}) is outside working hours ({ws}–{we})"
                            ),
                        });
                    }
                }
            }
        }

        results.push(UserAvailability {
            user_id,
            busy: !all_events.is_empty(),
            events: all_events,
            warnings,
        });
    }
    Ok(HttpResponse::Ok().json(results))
}
