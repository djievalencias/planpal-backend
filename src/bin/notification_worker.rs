/// Notification worker — handles all outbound notification delivery jobs.
///
/// Subscribed subjects:
///   {prefix}.jobs.notify_invitees
///   {prefix}.jobs.notify_organizer_rsvp
///   {prefix}.jobs.send_notification
use planpal::{logging, queue::{jobs, Job}, worker};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    logging::init_from_env();
    logging::info("Starting PlanPal notification-worker");

    let state = worker::bootstrap().await;

    worker::run(
        "notification-worker",
        state,
        &["notify_invitees", "notify_organizer_rsvp", "send_notification"],
        |state, job| Box::pin(dispatch(state, job)),
    )
    .await;
}

async fn dispatch(state: planpal::AppState, job: Job) -> Result<(), planpal::error::AppError> {
    match job {
        Job::NotifyInvitees { meeting_id } => {
            jobs::send_notification::notify_invitees(meeting_id, &state).await
        }
        Job::NotifyOrganizerRsvp { meeting_id, attendee_id, accepted } => {
            jobs::send_notification::notify_organizer_rsvp(meeting_id, attendee_id, accepted, &state).await
        }
        Job::SendNotification { meeting_id, channel } => {
            jobs::send_notification::run(meeting_id, channel, &state).await
        }
        other => {
            planpal::logging::warn_with(
                &[("job", &format!("{other:?}"))],
                "notification-worker received unhandled job type — skipping",
            );
            Ok(())
        }
    }
}
