/// Slack slash-command webhook handler.
///
/// Slack requires:
///   1. Verify the `X-Slack-Signature` HMAC-SHA256 before processing the body.
///   2. Respond within 3 seconds; use `response_url` for deferred replies.
use crate::{
    adapter::slack::commands::{self, SlackCommand},
    error::AppError,
    AppState,
};
use actix_web::{post, web, HttpRequest, HttpResponse};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// Slack sends slash commands as `application/x-www-form-urlencoded`.
#[derive(Deserialize, Debug)]
pub struct SlackPayload {
    pub command: String,
    pub text: String,
    pub user_id: String,
    pub user_name: Option<String>,
    pub team_id: String,
    pub channel_id: String,
    pub response_url: String,
    pub trigger_id: String,
}

#[post("/slack/events")]
pub async fn slack_events(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Bytes,
) -> Result<HttpResponse, AppError> {
    // ── Verify Slack signature ────────────────────────────────────────────────
    verify_slack_signature(&req, &body, &state.config.slack.signing_secret)?;

    // ── Parse form body ───────────────────────────────────────────────────────
    let payload: SlackPayload =
        serde_urlencoded::from_bytes(&body).map_err(|e| AppError::BadRequest(e.to_string()))?;

    // ── Dispatch command ──────────────────────────────────────────────────────
    let cmd = commands::parse(&payload.text);

    let response_body = match cmd {
        SlackCommand::Help => commands::help_response(),
        SlackCommand::Unknown(ref text) => commands::unknown_response(text),

        SlackCommand::Schedule {
            ref attendee_slack_ids,
            duration_minutes,
        } => {
            // Look up the requesting Slack user's PlanPal account by Slack user ID
            // (stored as google_sub or resolved via Slack API — simplified here)
            let ack = commands::ack_schedule_response(duration_minutes, attendee_slack_ids.len());

            // Fire-and-forget: create the meeting request and enqueue scheduling
            let state_clone = state.clone();
            let attendee_ids_clone = attendee_slack_ids.clone();
            let response_url = payload.response_url.clone();
            let requester_slack_id = payload.user_id.clone();
            let duration = duration_minutes;

            tokio::spawn(async move {
                if let Err(e) = handle_schedule_async(
                    &state_clone,
                    &requester_slack_id,
                    &attendee_ids_clone,
                    duration,
                    &response_url,
                )
                .await
                {
                    crate::logging::error_with(&[("error", &e.to_string())], "Slack schedule handler failed");
                }
            });

            ack
        }

        SlackCommand::Status { meeting_id } => {
            match crate::repository::meeting_repo::find_by_id(&state.db, meeting_id).await {
                Ok(Some(detail)) => {
                    let top = detail.proposals.first();
                    serde_json::json!({
                        "response_type": "ephemeral",
                        "text": format!(
                            "*{}* — status: `{:?}`\n{}",
                            detail.request.title,
                            detail.request.status,
                            top.map(|p| format!(
                                "Best slot: {} → {} (score: {:.2})",
                                p.proposed_start.format("%a %d %b %H:%M UTC"),
                                p.proposed_end.format("%H:%M"),
                                p.score
                            )).unwrap_or_else(|| "No proposals yet.".into())
                        )
                    })
                }
                _ => serde_json::json!({
                    "response_type": "ephemeral",
                    "text": format!("Meeting `{}` not found.", meeting_id)
                }),
            }
        }

        SlackCommand::Calendars => {
            serde_json::json!({
                "response_type": "ephemeral",
                "text": "Use the PlanPal web app to manage your connected calendars."
            })
        }
    };

    Ok(HttpResponse::Ok().json(response_body))
}

/// Verify `X-Slack-Signature` according to Slack's signing-secret protocol.
fn verify_slack_signature(
    req: &HttpRequest,
    body: &[u8],
    signing_secret: &str,
) -> Result<(), AppError> {
    let timestamp = req
        .headers()
        .get("X-Slack-Request-Timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("missing Slack timestamp header".into()))?;

    // Replay-attack guard: reject requests older than 5 minutes
    let ts: u64 = timestamp
        .parse()
        .map_err(|_| AppError::Unauthorized("invalid timestamp".into()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    if now.abs_diff(ts) > 300 {
        return Err(AppError::Unauthorized("stale Slack request".into()));
    }

    let expected_sig = req
        .headers()
        .get("X-Slack-Signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("missing Slack signature header".into()))?;

    let sig_base = format!("v0:{}:{}", timestamp, std::str::from_utf8(body).unwrap_or(""));

    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .map_err(|e| AppError::Internal(format!("HMAC init failed: {e}")))?;
    mac.update(sig_base.as_bytes());

    let computed = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

    if !constant_time_eq(computed.as_bytes(), expected_sig.as_bytes()) {
        return Err(AppError::Unauthorized("invalid Slack signature".into()));
    }

    Ok(())
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Async part of the schedule command: create meeting + enqueue scheduler.
async fn handle_schedule_async(
    _state: &AppState,
    _requester_slack_id: &str,
    _attendee_slack_ids: &[String],
    duration_minutes: u32,
    _response_url: &str,
) -> Result<(), AppError> {
    // TODO: Resolve Slack user IDs → PlanPal UUIDs via Slack users.info API
    // For now we demonstrate the flow with the meeting creation skeleton.
    crate::logging::info_with(
        &[("duration_minutes", &duration_minutes.to_string())],
        "Slack schedule command received — awaiting Slack→PlanPal user resolution",
    );
    Ok(())
}
