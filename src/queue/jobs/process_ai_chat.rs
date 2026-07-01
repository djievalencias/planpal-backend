use crate::{
    ai::prompt::{self, AiResponse, AttendeeBusySlot},
    error::AppError,
    logging,
    model::ai_chat::AiChatRole,
    model::meeting::RoomType,
    queue::{nats, Job},
    repository::{ai_chat_repo, event_repo, meeting_repo, user_repo},
    AppState,
};
use uuid::Uuid;

/// Process an AI chat message: call Bedrock, resolve users, and optionally
/// create a meeting.
pub async fn run(session_id: Uuid, _message_id: Uuid, state: &AppState) -> Result<(), AppError> {
    logging::info_with(
        &[("session_id", &session_id.to_string())],
        "process_ai_chat: starting",
    );

    let session = ai_chat_repo::find_session(&state.db, session_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("ai chat session {session_id}")))?;

    let requester = user_repo::find_by_id(&state.db, session.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("requester not found".into()))?;

    logging::info_with(
        &[
            ("session_id", &session_id.to_string()),
            ("requester", &requester.display_name),
            ("session_status", &format!("{:?}", session.status)),
        ],
        "process_ai_chat: session loaded",
    );

    let messages = ai_chat_repo::list_messages(&state.db, session_id).await?;

    // ── Prompt guard: prevent abuse and cost explosion ────────────────────
    let last_user_msg = messages.iter().rev().find(|m| m.role == AiChatRole::User);
    if let Some(msg) = last_user_msg {
        // Max message length: 500 chars
        if msg.content.len() > 500 {
            ai_chat_repo::insert_message(
                &state.db, session_id, &AiChatRole::Assistant,
                "Your message is too long. Could you keep it shorter? Just tell me who you want to meet, when, and for how long.",
                None,
            ).await?;
            return Ok(());
        }
    }

    // Max 20 messages per session (10 back-and-forth turns)
    if messages.len() > 20 {
        ai_chat_repo::insert_message(
            &state.db, session_id, &AiChatRole::Assistant,
            "This conversation has gotten quite long. Could you start a new chat and summarize what you need?",
            None,
        ).await?;
        return Ok(());
    }
    logging::info_with(
        &[
            ("session_id", &session_id.to_string()),
            ("message_count", &messages.len().to_string()),
        ],
        "process_ai_chat: conversation history loaded",
    );

    // Extract client timezone from latest user message (sent by frontend)
    let client_timezone: Option<String> = messages
        .iter()
        .rev()
        .find(|m| m.role == AiChatRole::User)
        .and_then(|m| m.metadata.as_ref())
        .and_then(|meta| meta.get("client_timezone"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Load previously resolved attendees from session metadata
    let resolved: Vec<(String, Uuid)> = session
        .metadata
        .get("resolved_attendees")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let now = chrono::Utc::now();
    // Look ahead 7 days for busy slots
    let window_start = now;
    let window_end = now + chrono::Duration::days(7);

    // Determine display timezone for busy slots in prompt
    let prompt_tz: chrono_tz::Tz = client_timezone
        .as_deref()
        .or(requester.timezone.as_deref())
        .and_then(|t| t.parse().ok())
        .unwrap_or(chrono_tz::UTC);
    let tz_label = client_timezone
        .as_deref()
        .or(requester.timezone.as_deref())
        .unwrap_or("UTC");

    let mut resolved_with_busy: Vec<(String, crate::model::user::User, Vec<AttendeeBusySlot>)> = Vec::new();
    for (raw_name, uid) in &resolved {
        if let Some(user) = user_repo::find_by_id(&state.db, *uid).await? {
            // Fetch busy slots for this attendee — display in requester's local timezone
            let busy_slots = event_repo::all_conflicts_for_user(
                &state.db, *uid, window_start, window_end,
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(title, start, end)| {
                let local_start = start.with_timezone(&prompt_tz);
                let local_end = end.with_timezone(&prompt_tz);
                AttendeeBusySlot {
                    title,
                    start: format!("{} {}", local_start.format("%Y-%m-%d %H:%M"), tz_label),
                    end: format!("{} {}", local_end.format("%H:%M"), tz_label),
                }
            })
            .collect();
            resolved_with_busy.push((raw_name.clone(), user, busy_slots));
        }
    }

    let prompt_attendees: Vec<(String, &crate::model::user::User, Vec<AttendeeBusySlot>)> =
        resolved_with_busy.iter().map(|(n, u, b)| (n.clone(), u, b.clone())).collect();
    let system_prompt = prompt::build_system_prompt(
        &requester,
        &prompt_attendees,
        &now,
        client_timezone.as_deref(),
    );

    let provider = state.ai_provider.as_ref().ok_or_else(|| {
        AppError::Internal("AI provider not initialised".into())
    })?;

    let ai_response = provider.converse(&system_prompt, &messages).await;

    // On any Bedrock/infra error, write a friendly message to the user
    let ai_response = match ai_response {
        Ok(r) => r,
        Err(e) => {
            logging::error_with(
                &[("session_id", &session_id.to_string()), ("error", &e.to_string())],
                "process_ai_chat: bedrock call failed, sending fallback to user",
            );
            let user_msg = "I'm having trouble processing your request right now. Please try again in a moment.";
            ai_chat_repo::insert_message(
                &state.db,
                session_id,
                &AiChatRole::Assistant,
                user_msg,
                None,
            )
            .await?;
            return Ok(());
        }
    };

    match ai_response {
        AiResponse::Clarify { message } => {
            logging::info_with(
                &[("session_id", &session_id.to_string())],
                "process_ai_chat: AI needs clarification",
            );
            ai_chat_repo::insert_message(
                &state.db,
                session_id,
                &AiChatRole::Assistant,
                &message,
                None,
            )
            .await?;
        }

        AiResponse::Suggest { message, suggestions } => {
            logging::info_with(
                &[
                    ("session_id", &session_id.to_string()),
                    ("attendee_options", &suggestions.attendees.len().to_string()),
                    ("time_options", &suggestions.time_slots.len().to_string()),
                ],
                "process_ai_chat: AI suggests options",
            );

            // Resolve attendee suggestions — look up real users and build metadata
            let mut attendee_options: Vec<serde_json::Value> = Vec::new();
            for raw in &suggestions.attendees {
                let clean = raw.split('(').next().unwrap_or(raw).trim();
                let email = raw.find('(')
                    .and_then(|s| raw.find(')').map(|e| raw[s+1..e].trim().to_string()))
                    .filter(|s| s.contains('@'));

                if let Some(ref email) = email {
                    if let Ok(Some(user)) = user_repo::find_by_email(&state.db, email).await {
                        attendee_options.push(serde_json::json!({
                            "id": user.id, "name": user.display_name, "email": user.email,
                        }));
                        continue;
                    }
                }
                let matches = user_repo::search_all(&state.db, clean, 5).await.unwrap_or_default();
                for u in matches.iter().filter(|u| u.id != requester.id) {
                    attendee_options.push(serde_json::json!({
                        "id": u.id, "name": u.display_name, "email": u.email,
                    }));
                }
            }

            // Validate time slot suggestions — filter out slots with conflicts
            let display_tz: chrono_tz::Tz = client_timezone.as_deref()
                .or(requester.timezone.as_deref())
                .and_then(|t| t.parse().ok())
                .unwrap_or(chrono_tz::UTC);

            // Collect all resolved attendee IDs for conflict checking
            let check_ids: Vec<Uuid> = {
                let mut ids: Vec<Uuid> = resolved.iter().map(|(_, uid)| *uid).collect();
                if !ids.contains(&requester.id) { ids.push(requester.id); }
                ids
            };

            let mut valid_slots: Vec<serde_json::Value> = Vec::new();
            for slot in &suggestions.time_slots {
                let slot_tz: chrono_tz::Tz = slot.timezone.parse().unwrap_or(display_tz);
                let start = chrono::NaiveDate::parse_from_str(&slot.date, "%Y-%m-%d").ok()
                    .and_then(|d| chrono::NaiveTime::parse_from_str(&slot.start_time, "%H:%M").ok().map(|t| (d, t)))
                    .and_then(|(d, t)| d.and_time(t).and_local_timezone(slot_tz).single())
                    .map(|dt| dt.with_timezone(&chrono::Utc));
                let end = chrono::NaiveDate::parse_from_str(&slot.date, "%Y-%m-%d").ok()
                    .and_then(|d| chrono::NaiveTime::parse_from_str(&slot.end_time, "%H:%M").ok().map(|t| (d, t)))
                    .and_then(|(d, t)| d.and_time(t).and_local_timezone(slot_tz).single())
                    .map(|dt| dt.with_timezone(&chrono::Utc));

                if let (Some(s), Some(e)) = (start, end) {
                    if s < chrono::Utc::now() || e <= s { continue; }

                    // Check conflicts for all known attendees
                    let mut has_conflict = false;
                    for &uid in &check_ids {
                        let conflicts = event_repo::all_conflicts_for_user(&state.db, uid, s, e)
                            .await.unwrap_or_default();
                        if !conflicts.is_empty() { has_conflict = true; break; }
                    }
                    if has_conflict { continue; }

                    let local_start = s.with_timezone(&display_tz);
                    let local_end = e.with_timezone(&display_tz);
                    valid_slots.push(serde_json::json!({
                        "date": local_start.format("%Y-%m-%d").to_string(),
                        "start_time": local_start.format("%H:%M").to_string(),
                        "end_time": local_end.format("%H:%M").to_string(),
                        "label": format!("{} – {}", local_start.format("%H:%M"), local_end.format("%H:%M")),
                    }));
                }
            }

            let metadata = serde_json::json!({
                "ambiguous_users": if attendee_options.is_empty() { serde_json::Value::Null } else { serde_json::Value::Array(attendee_options) },
                "time_slots": if valid_slots.is_empty() { serde_json::Value::Null } else { serde_json::Value::Array(valid_slots) },
            });

            ai_chat_repo::insert_message(
                &state.db, session_id, &AiChatRole::Assistant,
                &message, Some(metadata),
            ).await?;
        }

        AiResponse::Schedule { message, meeting } => {
            logging::info_with(
                &[
                    ("session_id", &session_id.to_string()),
                    ("title", &meeting.title),
                    ("date", &meeting.date),
                    ("start", &meeting.start_time),
                    ("attendees", &meeting.attendees.join(", ")),
                ],
                "process_ai_chat: AI wants to schedule",
            );

            // ── Resolve attendees ────────────────────────────────────────────
            let mut attendee_ids: Vec<Uuid> = Vec::new();
            let mut new_resolved: Vec<(String, Uuid)> = resolved.clone();
            let mut issues: Vec<String> = Vec::new();
            let mut ambiguous_users: Vec<serde_json::Value> = Vec::new();

            for raw_name in &meeting.attendees {
                // Clean up the name — AI sometimes outputs "Name (email)" format
                let clean_name = raw_name
                    .split('(').next().unwrap_or(raw_name)
                    .trim()
                    .to_string();
                // Also extract email if present: "Name (email@example.com)"
                let embedded_email = raw_name
                    .find('(')
                    .and_then(|start| raw_name.find(')').map(|end| &raw_name[start+1..end]))
                    .filter(|s| s.contains('@'))
                    .map(|s| s.trim().to_string());

                // Already resolved from a prior turn?
                if let Some((_, uid)) = resolved.iter().find(|(n, _)| n == raw_name || n == &clean_name) {
                    attendee_ids.push(*uid);
                    continue;
                }

                // Try embedded email first (exact match), then fall back to name search
                let all_matches = if let Some(ref email) = embedded_email {
                    if let Some(user) = user_repo::find_by_email(&state.db, email).await? {
                        vec![user]
                    } else {
                        user_repo::search_all(&state.db, &clean_name, 10).await?
                    }
                } else {
                    user_repo::search_all(&state.db, &clean_name, 10).await?
                };
                let others: Vec<_> = all_matches.iter().filter(|u| u.id != requester.id).collect();
                let self_match = all_matches.iter().any(|u| u.id == requester.id);

                if others.is_empty() && self_match {
                    issues.push(format!("'{}' is you — you're already included as organizer. Please specify someone else.", clean_name));
                } else if others.is_empty() {
                    issues.push(format!("I couldn't find anyone named '{}'. Please double-check the name or try their email.", clean_name));
                } else if others.len() == 1 {
                    attendee_ids.push(others[0].id);
                    new_resolved.push((clean_name.clone(), others[0].id));
                } else {
                    // Multiple matches — build button metadata
                    issues.push(format!("Which '{}' do you mean?", clean_name));
                    for u in &others {
                        ambiguous_users.push(serde_json::json!({
                            "id": u.id,
                            "name": u.display_name,
                            "email": u.email,
                        }));
                    }
                }
            }

            // If any attendee issues, send message with button metadata and stop
            if !issues.is_empty() {
                let mut meta = session.metadata.clone();
                meta["resolved_attendees"] = serde_json::to_value(&new_resolved).unwrap_or_default();
                ai_chat_repo::update_session_metadata(&state.db, session_id, meta).await?;

                let msg_metadata = if ambiguous_users.is_empty() {
                    None
                } else {
                    Some(serde_json::json!({ "ambiguous_users": ambiguous_users }))
                };
                ai_chat_repo::insert_message(
                    &state.db, session_id, &AiChatRole::Assistant,
                    &issues.join("\n\n"), msg_metadata,
                ).await?;
                return Ok(());
            }

            if attendee_ids.is_empty() || attendee_ids.iter().all(|id| *id == requester.id) {
                ai_chat_repo::insert_message(
                    &state.db, session_id, &AiChatRole::Assistant,
                    "Who would you like to invite to this meeting?", None,
                ).await?;
                return Ok(());
            }

            // Save resolved attendees
            let mut meta = session.metadata.clone();
            meta["resolved_attendees"] = serde_json::to_value(&new_resolved).unwrap_or_default();
            ai_chat_repo::update_session_metadata(&state.db, session_id, meta).await?;

            // ── Parse and validate date/time ─────────────────────────────────
            let tz: chrono_tz::Tz = meeting.timezone.parse().unwrap_or(chrono_tz::UTC);

            let start_dt = chrono::NaiveDate::parse_from_str(&meeting.date, "%Y-%m-%d").ok()
                .and_then(|d| chrono::NaiveTime::parse_from_str(&meeting.start_time, "%H:%M").ok().map(|t| (d, t)))
                .and_then(|(d, t)| d.and_time(t).and_local_timezone(tz).single())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let end_dt = chrono::NaiveDate::parse_from_str(&meeting.date, "%Y-%m-%d").ok()
                .and_then(|d| chrono::NaiveTime::parse_from_str(&meeting.end_time, "%H:%M").ok().map(|t| (d, t)))
                .and_then(|(d, t)| d.and_time(t).and_local_timezone(tz).single())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let (start_dt, end_dt) = match (start_dt, end_dt) {
                (Some(s), Some(e)) if e > s && s > chrono::Utc::now() => (s, e),
                _ => {
                    ai_chat_repo::insert_message(
                        &state.db, session_id, &AiChatRole::Assistant,
                        "I couldn't validate the proposed date/time. Could you try again with a specific date and time?",
                        None,
                    ).await?;
                    return Ok(());
                }
            };

            // ── Check conflicts ──────────────────────────────────────────────
            let display_tz: chrono_tz::Tz = client_timezone.as_deref()
                .or(requester.timezone.as_deref())
                .and_then(|t| t.parse().ok())
                .unwrap_or(tz);

            let mut all_check_ids = attendee_ids.clone();
            if !all_check_ids.contains(&requester.id) {
                all_check_ids.push(requester.id);
            }

            let mut conflict_lines: Vec<String> = Vec::new();
            for &uid in &all_check_ids {
                for (title, cs, ce) in event_repo::all_conflicts_for_user(&state.db, uid, start_dt, end_dt).await.unwrap_or_default() {
                    let name = if uid == requester.id { "You".into() } else {
                        user_repo::find_by_id(&state.db, uid).await.ok().flatten()
                            .map(|u| u.display_name).unwrap_or_else(|| "An attendee".into())
                    };
                    conflict_lines.push(format!(
                        "- {} has \"{}\" ({} – {})",
                        name, title,
                        cs.with_timezone(&display_tz).format("%H:%M"),
                        ce.with_timezone(&display_tz).format("%H:%M"),
                    ));
                }
            }
            if !conflict_lines.is_empty() {
                // Find alternative conflict-free slots as clickable buttons
                let duration = chrono::Duration::minutes(meeting.duration_minutes as i64);
                let ws: chrono::NaiveTime = chrono::NaiveTime::parse_from_str(
                    requester.work_start.as_deref().unwrap_or("08:00"), "%H:%M"
                ).unwrap_or(chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap());
                let we: chrono::NaiveTime = chrono::NaiveTime::parse_from_str(
                    requester.work_end.as_deref().unwrap_or("17:00"), "%H:%M"
                ).unwrap_or(chrono::NaiveTime::from_hms_opt(17, 0, 0).unwrap());

                let date = start_dt.with_timezone(&display_tz).date_naive();
                let mut alt_slots: Vec<serde_json::Value> = Vec::new();
                let mut candidate = ws;
                while candidate < we && alt_slots.len() < 4 {
                    if let Some(cs) = date.and_time(candidate)
                        .and_local_timezone(display_tz).single()
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                    {
                        let ce = cs + duration;
                        if cs >= chrono::Utc::now() {
                            let mut free = true;
                            for &uid in &all_check_ids {
                                if !event_repo::all_conflicts_for_user(&state.db, uid, cs, ce)
                                    .await.unwrap_or_default().is_empty() {
                                    free = false;
                                    break;
                                }
                            }
                            if free {
                                let ls = cs.with_timezone(&display_tz);
                                let le = ce.with_timezone(&display_tz);
                                alt_slots.push(serde_json::json!({
                                    "date": ls.format("%Y-%m-%d").to_string(),
                                    "start_time": ls.format("%H:%M").to_string(),
                                    "end_time": le.format("%H:%M").to_string(),
                                    "label": format!("{} – {}", ls.format("%H:%M"), le.format("%H:%M")),
                                }));
                            }
                        }
                    }
                    candidate += chrono::Duration::minutes(30);
                }

                let msg = format!(
                    "There are scheduling conflicts:\n{}\n\nPick an available time below, or tell me a different time:",
                    conflict_lines.join("\n")
                );
                let metadata = if alt_slots.is_empty() { None } else {
                    Some(serde_json::json!({ "time_slots": alt_slots }))
                };
                ai_chat_repo::insert_message(&state.db, session_id, &AiChatRole::Assistant, &msg, metadata).await?;
                return Ok(());
            }

            // ── All validated — create the meeting ───────────────────────────
            let mut all_attendee_ids = vec![requester.id];
            for id in &attendee_ids {
                if *id != requester.id { all_attendee_ids.push(*id); }
            }

            // Strip placeholder/fake URLs the AI might generate
            let room_link = meeting.room_link.as_deref()
                .filter(|l| !l.contains("meet.google.com/landing"))
                .filter(|l| !l.contains("example.com"))
                .filter(|l| l.starts_with("https://") || l.starts_with("http://"))
                .map(|l| l.to_string());

            // Parse room type from AI response
            let room_type = match meeting.room_type.as_deref() {
                Some("zoom") => RoomType::Zoom,
                Some("gmeet") => RoomType::Gmeet,
                Some("teams") => RoomType::Teams,
                Some("phone") => RoomType::Phone,
                Some("in_person") => RoomType::InPerson,
                _ => RoomType::None,
            };

            let created = match meeting_repo::create(&state.db, meeting_repo::NewMeetingRequest {
                requester_id: requester.id,
                title: meeting.title.clone(),
                description: meeting.description.clone(),
                duration_minutes: meeting.duration_minutes,
                preferred_window_start: start_dt,
                preferred_window_end: end_dt,
                attendee_ids: all_attendee_ids,
                room_type,
                room_link: room_link.clone(),
                location: meeting.location.clone(),
            }).await {
                Ok(m) => m,
                Err(e) => {
                    logging::error_with(
                        &[("error", &e.to_string()), ("session_id", &session_id.to_string())],
                        "process_ai_chat: meeting creation failed",
                    );
                    ai_chat_repo::insert_message(
                        &state.db, session_id, &AiChatRole::Assistant,
                        "I wasn't able to create the meeting. Please try again.", None,
                    ).await?;
                    return Ok(());
                }
            };

            logging::info_with(
                &[
                    ("session_id", &session_id.to_string()),
                    ("meeting_id", &created.id.to_string()),
                ],
                "process_ai_chat: meeting created",
            );

            // Auto-generate Google Meet link if room_type is gmeet and no real link provided
            let is_gmeet = matches!(meeting.room_type.as_deref(), Some("gmeet"));
            let mut meet_link: Option<String> = None;
            if is_gmeet && room_link.is_none() {
                let providers = crate::repository::calendar_repo::list_for_user(&state.db, requester.id)
                    .await.unwrap_or_default();
                if let Some(gcal) = providers.iter().find(|p| p.kind == crate::model::calendar::ProviderKind::GoogleCalendar) {
                    // Refresh token if expired
                    let mut gcal_provider = crate::provider::gcal::GoogleCalendarProvider {
                        provider: gcal.clone(),
                        http: state.http.clone(),
                        google_config: state.config.google.clone(),
                        pool: state.db.clone(),
                    };
                    let _ = gcal_provider.ensure_fresh_token().await;
                    let token = gcal_provider.provider.access_token.as_deref().unwrap_or("");
                    let emails: Vec<String> = attendee_ids.iter()
                        .filter_map(|uid| {
                            // We already have resolved users — look them up
                            resolved_with_busy.iter().find(|(_, u, _)| u.id == *uid).map(|(_, u, _)| u.email.clone())
                        })
                        .collect();
                    match crate::provider::gcal::create_meeting_event(
                        &state.http, token, &meeting.title,
                        start_dt, end_dt, &emails, true, "tentative",
                    ).await {
                        Ok(result) => {
                            if let Some(ref link) = result.meet_link {
                                let _ = meeting_repo::update_room_link(&state.db, created.id, link).await;
                                meet_link = Some(link.clone());
                                logging::info_with(
                                    &[("meeting_id", &created.id.to_string()), ("meet_link", link)],
                                    "process_ai_chat: auto-generated Google Meet link",
                                );
                            }
                        }
                        Err(e) => {
                            logging::warn_with(
                                &[("error", &e.to_string())],
                                "process_ai_chat: failed to auto-generate Meet link",
                            );
                        }
                    }
                }
            }

            // Publish scheduling + notification jobs (best-effort — meeting is already created)
            let _ = nats::publish(
                &state.nats,
                &state.config.nats,
                &Job::ScheduleMeeting {
                    meeting_request_id: created.id,
                },
                &state.metrics,
            )
            .await;

            let _ = nats::publish(
                &state.nats,
                &state.config.nats,
                &Job::NotifyInvitees {
                    meeting_id: created.id,
                },
                &state.metrics,
            )
            .await;

            // Mark session as completed
            let _ = ai_chat_repo::complete_session(&state.db, session_id, created.id).await;

            // Write confirmation message (include Meet link if generated)
            let confirm_msg = if let Some(ref link) = meet_link {
                format!("{}\n\nGoogle Meet link: {}", message, link)
            } else {
                message.clone()
            };
            ai_chat_repo::insert_message(
                &state.db,
                session_id,
                &AiChatRole::Assistant,
                &confirm_msg,
                Some(serde_json::json!({
                    "meeting_id": created.id,
                    "status": "created",
                    "meet_link": meet_link,
                })),
            )
            .await?;
        }
    }

    logging::info_with(
        &[("session_id", &session_id.to_string())],
        "process_ai_chat: done",
    );
    Ok(())
}
