use crate::{
    error::AppError,
    model::meeting::MeetingStatus,
    queue::{nats, Job},
    repository::{event_repo, meeting_repo},
    scheduler::suggest,
    AppState,
};
use crate::model::notification::NotificationChannel;
use uuid::Uuid;

/// Find available time slots for all attendees, persist proposals to DB,
/// record per-attendee conflicts, and dual-write the result to Redis.
pub async fn run(meeting_request_id: Uuid, state: &AppState) -> Result<(), AppError> {
    let detail = meeting_repo::find_by_id(&state.db, meeting_request_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting request {meeting_request_id}")))?;

    let attendee_ids: Vec<_> = detail.attendees.iter().map(|a| a.user_id).collect();

    // ── Record per-attendee conflicts (best-effort, non-fatal) ────────────────
    for &uid in &attendee_ids {
        let conflicts = event_repo::conflict_events_for_user(
            &state.db,
            uid,
            detail.request.preferred_window_start,
            detail.request.preferred_window_end,
        )
        .await
        .unwrap_or_default();

        if let Some((title, _, _)) = conflicts.into_iter().next() {
            let _ = meeting_repo::set_attendee_conflict(
                &state.db,
                meeting_request_id,
                uid,
                &title,
            )
            .await;
        }
    }

    // ── Find optimal time slots ───────────────────────────────────────────────
    let proposals =
        suggest::generate_proposals(&state.db, &detail.request, &attendee_ids).await?;

    if proposals.is_empty() {
        crate::logging::warn_with(
            &[("meeting_id", &meeting_request_id.to_string())],
            "no available slots found",
        );
        return Ok(());
    }

    meeting_repo::insert_proposals(&state.db, meeting_request_id, proposals).await?;
    meeting_repo::update_status(&state.db, meeting_request_id, MeetingStatus::ProposalReady).await?;

    // ── Dual-write to Redis (best-effort, DB is source of truth) ─────────────
    if let Some(ref redis) = state.redis {
        match meeting_repo::find_by_id(&state.db, meeting_request_id).await {
            Ok(Some(updated)) => {
                match serde_json::to_string(&updated) {
                    Ok(json) => {
                        let key = format!("meeting:{meeting_request_id}");
                        // TTL: 24 hours
                        let result: redis::RedisResult<()> = redis::cmd("SET")
                            .arg(&key)
                            .arg(&json)
                            .arg("EX")
                            .arg(86_400u64)
                            .query_async(&mut redis.clone())
                            .await;
                        if let Err(e) = result {
                            crate::logging::warn_with(
                                &[("error", &e.to_string()), ("meeting_id", &meeting_request_id.to_string())],
                                "Redis write failed (non-fatal)",
                            );
                        }
                    }
                    Err(e) => crate::logging::warn_with(
                        &[("error", &e.to_string())],
                        "JSON serialise for Redis failed",
                    ),
                }
            }
            _ => {}
        }
    }

    // ── Notify requester so they can pick a slot ──────────────────────────────
    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::SendNotification {
            meeting_id: meeting_request_id,
            channel: NotificationChannel::Email,
        },
        &state.metrics,
    )
    .await?;

    crate::logging::info_with(
        &[("meeting_id", &meeting_request_id.to_string())],
        "scheduling complete, proposals ready",
    );

    Ok(())
}
