use crate::{
    auth::middleware::AuthenticatedUser,
    error::AppError,
    model::meeting::RoomType,
    queue::{nats, Job},
    repository::meeting_repo::{self, NewMeetingRequest},
    AppState,
};
use actix_web::{delete, get, post, web, HttpResponse};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create_meeting)
        .service(list_meetings)
        .service(get_meeting)
        .service(confirm_meeting)
        .service(rsvp_meeting)
        .service(cancel_meeting);
}

#[derive(Deserialize, Validate)]
struct CreateMeetingRequest {
    #[validate(length(min = 1, max = 200))]
    title: String,
    description: Option<String>,
    #[validate(range(min = 15, max = 480))]
    duration_minutes: i32,
    attendee_ids: Vec<Uuid>,
    preferred_window_start: DateTime<Utc>,
    preferred_window_end: DateTime<Utc>,
    /// "zoom" | "gmeet" | "teams" | "phone" | "in_person" | "none" (default)
    #[serde(default)]
    room_type: Option<String>,
    /// Pre-generated or manually entered meeting link (Zoom/GMeet/Teams)
    room_link: Option<String>,
    /// Physical location for in_person meetings
    location: Option<String>,
}

fn parse_room_type(s: &str) -> Result<RoomType, AppError> {
    match s {
        "zoom"      => Ok(RoomType::Zoom),
        "gmeet"     => Ok(RoomType::Gmeet),
        "teams"     => Ok(RoomType::Teams),
        "phone"     => Ok(RoomType::Phone),
        "in_person" => Ok(RoomType::InPerson),
        "none"      => Ok(RoomType::None),
        other       => Err(AppError::BadRequest(format!("unknown room_type: {other}"))),
    }
}

#[post("/meetings")]
async fn create_meeting(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    body: web::Json<CreateMeetingRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    if body.preferred_window_end <= body.preferred_window_start {
        return Err(AppError::BadRequest("window_end must be after window_start".into()));
    }

    let room_type = parse_room_type(
        body.room_type.as_deref().unwrap_or("none"),
    )?;

    let meeting = meeting_repo::create(
        &state.db,
        NewMeetingRequest {
            requester_id: auth.0.id,
            title: body.title.clone(),
            description: body.description.clone(),
            duration_minutes: body.duration_minutes,
            preferred_window_start: body.preferred_window_start,
            preferred_window_end: body.preferred_window_end,
            attendee_ids: body.attendee_ids.clone(),
            room_type,
            room_link: body.room_link.clone(),
            location: body.location.clone(),
        },
    )
    .await?;

    // Enqueue scheduling job — worker handles free-slot search + DB/Redis write
    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::ScheduleMeeting { meeting_request_id: meeting.id },
        &state.metrics,
    )
    .await?;

    // Create tentative event on organizer's Google Calendar (if connected)
    {
        let providers = crate::repository::calendar_repo::list_for_user(&state.db, auth.0.id)
            .await.unwrap_or_default();
        if let Some(gcal) = providers.iter().find(|p| p.kind == crate::model::calendar::ProviderKind::GoogleCalendar) {
            let token = gcal.access_token.as_deref().unwrap_or("");
            let with_meet = body.room_type.as_deref() == Some("gmeet");
            // Look up attendee emails
            let mut emails = Vec::new();
            for uid in &body.attendee_ids {
                if let Ok(Some(u)) = crate::repository::user_repo::find_by_id(&state.db, *uid).await {
                    emails.push(u.email);
                }
            }
            match crate::provider::gcal::create_meeting_event(
                &state.http, token, &body.title,
                body.preferred_window_start, body.preferred_window_end,
                &emails, with_meet, "tentative",
            ).await {
                Ok(result) => {
                    let _ = crate::repository::gcal_mapping_repo::create(
                        &state.db, meeting.id, auth.0.id, gcal.id, &result.event_id,
                    ).await;
                    crate::logging::info_with(
                        &[("meeting_id", &meeting.id.to_string()), ("gcal_event_id", &result.event_id)],
                        "created tentative Google Calendar event",
                    );
                }
                Err(e) => crate::logging::warn_with(
                    &[("error", &e.to_string())],
                    "failed to create tentative Google Calendar event",
                ),
            }
        }
    }

    // Notify invitees of the new meeting invitation
    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::NotifyInvitees { meeting_id: meeting.id },
        &state.metrics,
    )
    .await?;

    Ok(HttpResponse::Created().json(meeting))
}

#[get("/meetings")]
async fn list_meetings(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
) -> Result<HttpResponse, AppError> {
    let meetings = meeting_repo::list_for_user(&state.db, auth.0.id).await?;
    Ok(HttpResponse::Ok().json(meetings))
}

#[get("/meetings/{id}")]
async fn get_meeting(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let detail = meeting_repo::find_by_id(&state.db, *path)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {}", *path)))?;

    if !detail.attendees.iter().any(|a| a.user_id == auth.0.id) {
        return Err(AppError::Forbidden("not an attendee of this meeting".into()));
    }

    Ok(HttpResponse::Ok().json(detail))
}

#[derive(Deserialize)]
struct ConfirmRequest {
    proposal_id: Uuid,
}

#[post("/meetings/{id}/confirm")]
async fn confirm_meeting(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
    body: web::Json<ConfirmRequest>,
) -> Result<HttpResponse, AppError> {
    let meeting_id = *path;
    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    // Must be an attendee
    if !detail.attendees.iter().any(|a| a.user_id == auth.0.id) {
        return Err(AppError::Forbidden("not an attendee of this meeting".into()));
    }
    // The organizer cannot self-confirm — an invitee must accept
    if detail.request.requester_id == auth.0.id {
        return Err(AppError::Forbidden("the organizer cannot confirm their own meeting; an invitee must accept".into()));
    }

    let ok = meeting_repo::select_proposal(&state.db, body.proposal_id, meeting_id).await?;
    if !ok {
        return Err(AppError::NotFound("proposal not found".into()));
    }

    // ── Push confirmed meeting to ALL attendees' Google Calendars ───────
    if let Some(proposal) = detail.proposals.iter().find(|p| p.id == body.proposal_id) {
        let with_meet = detail.request.room_type == crate::model::meeting::RoomType::Gmeet;
        let emails: Vec<String> = detail.attendees.iter().map(|a| a.email.clone()).collect();
        let existing_mappings = crate::repository::gcal_mapping_repo::find_by_meeting(&state.db, meeting_id)
            .await.unwrap_or_default();

        for attendee in &detail.attendees {
            let providers = crate::repository::calendar_repo::list_for_user(
                &state.db, attendee.user_id,
            ).await.unwrap_or_default();

            let Some(gcal) = providers.iter().find(|p| p.kind == crate::model::calendar::ProviderKind::GoogleCalendar) else {
                continue;
            };
            let token = gcal.access_token.as_deref().unwrap_or("");

            // Check if this user already has a tentative event (from creation) → update it
            let existing = existing_mappings.iter().find(|m| m.user_id == attendee.user_id);

            if let Some(mapping) = existing {
                // Update existing tentative → confirmed
                match crate::provider::gcal::update_event(
                    &state.http, token, &mapping.gcal_event_id,
                    &detail.request.title, proposal.proposed_start, proposal.proposed_end,
                    &emails, "confirmed",
                ).await {
                    Ok(_) => crate::logging::info_with(
                        &[("meeting_id", &meeting_id.to_string()), ("user_id", &attendee.user_id.to_string())],
                        "updated Google Calendar event to confirmed",
                    ),
                    Err(e) => crate::logging::warn_with(
                        &[("error", &e.to_string()), ("user_id", &attendee.user_id.to_string())],
                        "failed to update Google Calendar event",
                    ),
                }
                continue;
            }

            // No existing mapping → create new confirmed event
            match crate::provider::gcal::create_meeting_event(
                &state.http, token,
                &detail.request.title, proposal.proposed_start, proposal.proposed_end,
                &emails, with_meet, "confirmed",
            ).await {
                Ok(result) => {
                    let _ = crate::repository::gcal_mapping_repo::create(
                        &state.db, meeting_id, attendee.user_id, gcal.id, &result.event_id,
                    ).await;

                    if let Some(ref link) = result.meet_link {
                        if detail.request.room_link.is_none() {
                            let _ = meeting_repo::update_room_link(&state.db, meeting_id, link).await;
                        }
                    }

                    crate::logging::info_with(
                        &[
                            ("meeting_id", &meeting_id.to_string()),
                            ("user_id", &attendee.user_id.to_string()),
                            ("gcal_event_id", &result.event_id),
                        ],
                        "pushed confirmed meeting to Google Calendar",
                    );
                }
                Err(e) => {
                    crate::logging::warn_with(
                        &[
                            ("meeting_id", &meeting_id.to_string()),
                            ("user_id", &attendee.user_id.to_string()),
                            ("error", &e.to_string()),
                        ],
                        "failed to push meeting to Google Calendar",
                    );
                }
            }
        }
    }

    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::SendNotification {
            meeting_id,
            channel: crate::model::notification::NotificationChannel::Email,
        },
        &state.metrics,
    )
    .await?;

    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::SendNotification {
            meeting_id,
            channel: crate::model::notification::NotificationChannel::Push,
        },
        &state.metrics,
    )
    .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({"confirmed": true})))
}

#[derive(Deserialize)]
struct RsvpRequest {
    accepted: bool,
}

#[post("/meetings/{id}/rsvp")]
async fn rsvp_meeting(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
    body: web::Json<RsvpRequest>,
) -> Result<HttpResponse, AppError> {
    let meeting_id = *path;
    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    // Must be an invitee (not the organizer)
    if !detail.attendees.iter().any(|a| a.user_id == auth.0.id) {
        return Err(AppError::Forbidden("not an attendee of this meeting".into()));
    }
    if detail.request.requester_id == auth.0.id {
        return Err(AppError::Forbidden("organizer cannot RSVP to their own meeting".into()));
    }

    let found = meeting_repo::update_rsvp(&state.db, meeting_id, auth.0.id, body.accepted).await?;
    if !found {
        return Err(AppError::NotFound("attendee record not found".into()));
    }

    // Notify the organizer of the response
    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::NotifyOrganizerRsvp {
            meeting_id,
            attendee_id: auth.0.id,
            accepted: body.accepted,
        },
        &state.metrics,
    )
    .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "accepted": body.accepted })))
}

#[delete("/meetings/{id}")]
async fn cancel_meeting(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let meeting_id = *path;
    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    if detail.request.requester_id != auth.0.id {
        return Err(AppError::Forbidden("only the requester can cancel".into()));
    }

    meeting_repo::update_status(
        &state.db,
        meeting_id,
        crate::model::meeting::MeetingStatus::Cancelled,
    )
    .await?;

    // ── Delete Google Calendar events for all attendees (best-effort) ─────
    let mappings = crate::repository::gcal_mapping_repo::find_by_meeting(&state.db, meeting_id)
        .await
        .unwrap_or_default();
    for mapping in &mappings {
        let providers = crate::repository::calendar_repo::list_for_user(&state.db, mapping.user_id)
            .await
            .unwrap_or_default();
        if let Some(gcal) = providers.iter().find(|p| p.id == mapping.provider_id) {
            let token = gcal.access_token.as_deref().unwrap_or("");
            match crate::provider::gcal::delete_event(&state.http, token, &mapping.gcal_event_id).await {
                Ok(_) => crate::logging::info_with(
                    &[("meeting_id", &meeting_id.to_string()), ("user_id", &mapping.user_id.to_string())],
                    "deleted Google Calendar event on cancel",
                ),
                Err(e) => crate::logging::warn_with(
                    &[("meeting_id", &meeting_id.to_string()), ("error", &e.to_string())],
                    "failed to delete Google Calendar event on cancel",
                ),
            }
        }
    }
    let _ = crate::repository::gcal_mapping_repo::delete_by_meeting(&state.db, meeting_id).await;

    Ok(HttpResponse::NoContent().finish())
}
