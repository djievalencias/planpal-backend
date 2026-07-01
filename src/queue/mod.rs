pub mod jobs;
pub mod nats;

use crate::model::notification::NotificationChannel;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All background jobs are represented as variants of this enum.
/// Serialised to JSON before being published to NATS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Job {
    // ── Scheduling ────────────────────────────────────────────────────────────
    SyncCalendar {
        provider_id: Uuid,
    },
    ScheduleMeeting {
        meeting_request_id: Uuid,
    },

    // ── Notifications ─────────────────────────────────────────────────────────
    /// Send a push notification to all invitees when a meeting is first created.
    NotifyInvitees {
        meeting_id: Uuid,
    },
    /// Notify the organizer when an invitee accepts or rejects.
    NotifyOrganizerRsvp {
        meeting_id: Uuid,
        attendee_id: Uuid,
        accepted: bool,
    },
    /// Legacy: fan-out email/push when a time slot is confirmed.
    SendNotification {
        meeting_id: Uuid,
        channel: NotificationChannel,
    },

    // ── AI Chat ──────────────────────────────────────────────────────────────
    /// Process a user message in an AI chat session via Bedrock.
    ProcessAiChat {
        session_id: Uuid,
        message_id: Uuid,
    },
}

impl Job {
    /// NATS subject for this job variant.
    pub fn subject(&self, prefix: &str) -> String {
        format!("{prefix}.jobs.{}", self.name())
    }

    /// Short identifier used for metrics labels and span attributes.
    pub fn name(&self) -> &'static str {
        match self {
            Job::SyncCalendar { .. }        => "sync_calendar",
            Job::ScheduleMeeting { .. }     => "schedule_meeting",
            Job::NotifyInvitees { .. }      => "notify_invitees",
            Job::NotifyOrganizerRsvp { .. } => "notify_organizer_rsvp",
            Job::SendNotification { .. }    => "send_notification",
            Job::ProcessAiChat { .. }      => "process_ai_chat",
        }
    }
}
