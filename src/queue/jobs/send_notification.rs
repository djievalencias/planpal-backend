use crate::{
    error::AppError,
    logging,
    model::notification::{NotificationChannel, NotificationType},
    notification::{email, push},
    repository::{meeting_repo, notification_repo, user_repo},
    AppState,
};
use uuid::Uuid;

// ── NotifyInvitees ────────────────────────────────────────────────────────────

/// Send a push invitation to every invitee (excluding the organizer) when a
/// meeting request is first created.
pub async fn notify_invitees(meeting_id: Uuid, state: &AppState) -> Result<(), AppError> {
    logging::info_with(
        &[("meeting_id", &meeting_id.to_string())],
        "notify_invitees: starting",
    );

    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    let organizer = user_repo::find_by_id(&state.db, detail.request.requester_id)
        .await?
        .ok_or_else(|| AppError::NotFound("organizer not found".into()))?;

    let invitees: Vec<_> = detail.attendees.iter()
        .filter(|a| a.user_id != detail.request.requester_id)
        .collect();
    logging::info_with(
        &[("meeting_id", &meeting_id.to_string()), ("invitee_count", &invitees.len().to_string())],
        "notify_invitees: sending push to invitees",
    );

    for attendee in invitees {
        let Some(user) = user_repo::find_by_id(&state.db, attendee.user_id).await? else {
            continue;
        };

        let payload = serde_json::json!({
            "meeting_id": meeting_id,
            "title": detail.request.title,
            "organizer": organizer.display_name,
        });

        let notif_id = notification_repo::create(
            &state.db,
            attendee.user_id,
            Some(meeting_id),
            &NotificationChannel::Push,
            &NotificationType::MeetingInvitation,
            payload,
        )
        .await?;

        // FCM push is best-effort — the in-app notification is delivered
        // by storing the row in the DB regardless of whether FCM succeeds.
        if let Some(token) = &user.fcm_token {
            logging::info_with(
                &[("user_id", &attendee.user_id.to_string()), ("event", "meeting_invitation")],
                "fcm: sending push notification",
            );
            let result = push::send_meeting_invitation(
                &state.http,
                &state.config.fcm,
                token,
                &detail.request.title,
                &organizer.display_name,
            ).await;
            let push_status = if result.is_ok() { "ok" } else { "error" };
            state.metrics.push_notification_sends_total
                .with_label_values(&["meeting_invitation", push_status])
                .inc();
            match &result {
                Ok(_) => logging::info_with(
                    &[("user_id", &attendee.user_id.to_string()), ("event", "meeting_invitation"), ("status", "ok")],
                    "fcm: push notification sent successfully",
                ),
                Err(e) => logging::error_with(
                    &[("error", &e.to_string()), ("user_id", &attendee.user_id.to_string()), ("event", "meeting_invitation"), ("status", "error")],
                    "fcm: push notification failed",
                ),
            }
        } else {
            logging::info_with(
                &[("user_id", &attendee.user_id.to_string())],
                "fcm: skipped — no fcm_token registered",
            );
        }

        notification_repo::mark_sent(&state.db, notif_id).await?;
    }

    logging::info_with(
        &[("meeting_id", &meeting_id.to_string())],
        "notify_invitees: done",
    );
    Ok(())
}

// ── NotifyOrganizerRsvp ───────────────────────────────────────────────────────

/// Notify the meeting organizer that an invitee has accepted or rejected.
pub async fn notify_organizer_rsvp(
    meeting_id: Uuid,
    attendee_id: Uuid,
    accepted: bool,
    state: &AppState,
) -> Result<(), AppError> {
    logging::info_with(
        &[
            ("meeting_id", &meeting_id.to_string()),
            ("attendee_id", &attendee_id.to_string()),
            ("accepted", &accepted.to_string()),
        ],
        "notify_organizer_rsvp: starting",
    );

    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    let attendee = user_repo::find_by_id(&state.db, attendee_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("attendee {attendee_id}")))?;

    let organizer = user_repo::find_by_id(&state.db, detail.request.requester_id)
        .await?
        .ok_or_else(|| AppError::NotFound("organizer not found".into()))?;

    let payload = serde_json::json!({
        "meeting_id": meeting_id,
        "title": detail.request.title,
        "attendee_id": attendee_id,
        "attendee_name": attendee.display_name,
        "accepted": accepted,
    });

    let notif_id = notification_repo::create(
        &state.db,
        organizer.id,
        Some(meeting_id),
        &NotificationChannel::Push,
        &NotificationType::AttendeeResponded,
        payload,
    )
    .await?;

    if let Some(token) = &organizer.fcm_token {
        logging::info_with(
            &[("organizer_id", &organizer.id.to_string()), ("event", "organizer_rsvp")],
            "fcm: sending push notification",
        );
        let result = push::send_rsvp_response(
            &state.http,
            &state.config.fcm,
            token,
            &detail.request.title,
            &attendee.display_name,
            accepted,
        ).await;
        let push_status = if result.is_ok() { "ok" } else { "error" };
        state.metrics.push_notification_sends_total
            .with_label_values(&["organizer_rsvp", push_status])
            .inc();
        match &result {
            Ok(_) => logging::info_with(
                &[("organizer_id", &organizer.id.to_string()), ("event", "organizer_rsvp"), ("status", "ok")],
                "fcm: push notification sent successfully",
            ),
            Err(e) => logging::error_with(
                &[("error", &e.to_string()), ("organizer_id", &organizer.id.to_string()), ("event", "organizer_rsvp"), ("status", "error")],
                "fcm: push notification failed",
            ),
        }
    } else {
        logging::info_with(
            &[("organizer_id", &organizer.id.to_string())],
            "fcm: skipped — no fcm_token registered",
        );
    }

    notification_repo::mark_sent(&state.db, notif_id).await?;

    logging::info_with(
        &[("meeting_id", &meeting_id.to_string())],
        "notify_organizer_rsvp: done",
    );
    Ok(())
}

// ── SendNotification (confirmed fan-out) ─────────────────────────────────────

/// Fan out email and/or push notifications to all attendees when a meeting
/// time slot is confirmed.
pub async fn run(
    meeting_id: Uuid,
    channel: NotificationChannel,
    state: &AppState,
) -> Result<(), AppError> {
    let channel_str = match channel {
        NotificationChannel::Email => "email",
        NotificationChannel::Push => "push",
    };
    logging::info_with(
        &[("meeting_id", &meeting_id.to_string()), ("channel", channel_str)],
        "send_notification: starting fan-out",
    );

    let detail = meeting_repo::find_by_id(&state.db, meeting_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("meeting {meeting_id}")))?;

    let proposal = detail
        .proposals
        .iter()
        .find(|p| p.is_selected)
        .or_else(|| detail.proposals.first());

    let (start, end) = match proposal {
        Some(p) => (p.proposed_start, p.proposed_end),
        None => {
            logging::warn_with(
                &[("meeting_id", &meeting_id.to_string()), ("channel", channel_str)],
                "send_notification: no proposal found — skipping",
            );
            return Ok(());
        }
    };

    logging::info_with(
        &[
            ("meeting_id", &meeting_id.to_string()),
            ("channel", channel_str),
            ("attendee_count", &detail.attendees.len().to_string()),
        ],
        "send_notification: delivering to attendees",
    );

    for attendee in &detail.attendees {
        let Some(user) = user_repo::find_by_id(&state.db, attendee.user_id).await? else {
            continue;
        };

        let payload = serde_json::json!({
            "meeting_id": meeting_id,
            "title": detail.request.title,
            "start": start,
            "end": end,
        });

        let notif_id = notification_repo::create(
            &state.db,
            attendee.user_id,
            Some(meeting_id),
            &channel,
            &NotificationType::MeetingConfirmed,
            payload,
        )
        .await?;

        let delivery_result = match channel {
            NotificationChannel::Email => {
                use crate::config::EmailProvider;

                let provider = state.config.email.provider();
                logging::info_with(
                    &[
                        ("to", &user.email),
                        ("provider", &provider.to_string()),
                        ("meeting_id", &meeting_id.to_string()),
                        ("user_id", &attendee.user_id.to_string()),
                    ],
                    "email: sending meeting notification",
                );
                let result = match provider {
                    EmailProvider::Ses => {
                        let ses = state.ses_client.as_ref().expect(
                            "SES client not initialised — set APP__EMAIL__PROVIDER=ses",
                        );
                        email::send_meeting_invitation_ses(
                            ses,
                            &state.config.smtp.from,
                            &user.email,
                            &user.display_name,
                            &detail.request.title,
                            &start,
                            &end,
                            None,
                        )
                        .await
                    }
                    EmailProvider::Smtp => {
                        email::send_meeting_invitation(
                            &state.mailer,
                            &state.config.smtp.from,
                            &user.email,
                            &user.display_name,
                            &detail.request.title,
                            &start,
                            &end,
                            None,
                        )
                        .await
                    }
                };
                let send_status = if result.is_ok() { "ok" } else { "error" };
                state.metrics.email_sends_total.with_label_values(&[send_status]).inc();
                match &result {
                    Ok(_) => logging::info_with(
                        &[
                            ("to", &user.email),
                            ("provider", &provider.to_string()),
                            ("status", "ok"),
                        ],
                        "email: sent successfully",
                    ),
                    Err(e) => logging::error_with(
                        &[
                            ("error", &e.to_string()),
                            ("to", &user.email),
                            ("provider", &provider.to_string()),
                            ("user_id", &attendee.user_id.to_string()),
                            ("status", "error"),
                        ],
                        "email: send failed",
                    ),
                }
                result
            }
            NotificationChannel::Push => {
                if let Some(token) = &user.fcm_token {
                    logging::info_with(
                        &[("user_id", &attendee.user_id.to_string()), ("event", "meeting_confirmed")],
                        "fcm: sending push notification",
                    );
                    let result = push::send_meeting_confirmed(
                        &state.http,
                        &state.config.fcm,
                        token,
                        &detail.request.title,
                        &start,
                    )
                    .await;
                    let send_status = if result.is_ok() { "ok" } else { "error" };
                    state.metrics.push_notification_sends_total
                        .with_label_values(&["meeting_confirmed", send_status])
                        .inc();
                    match &result {
                        Ok(_) => logging::info_with(
                            &[("user_id", &attendee.user_id.to_string()), ("event", "meeting_confirmed"), ("status", "ok")],
                            "fcm: push notification sent successfully",
                        ),
                        Err(e) => logging::error_with(
                            &[("error", &e.to_string()), ("user_id", &attendee.user_id.to_string()), ("event", "meeting_confirmed"), ("status", "error")],
                            "fcm: push notification failed",
                        ),
                    }
                    result
                } else {
                    logging::info_with(
                        &[("user_id", &attendee.user_id.to_string())],
                        "fcm: skipped — no fcm_token registered",
                    );
                    Ok(())
                }
            }
        };

        match delivery_result {
            Ok(_) => notification_repo::mark_sent(&state.db, notif_id).await?,
            Err(e) => notification_repo::mark_failed(&state.db, notif_id, &e.to_string()).await?,
        }
    }

    logging::info_with(
        &[("meeting_id", &meeting_id.to_string()), ("channel", channel_str)],
        "send_notification: fan-out complete",
    );
    Ok(())
}
