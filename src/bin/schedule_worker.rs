/// Schedule worker — handles calendar sync and meeting scheduling jobs.
///
/// Subscribed subjects:
///   {prefix}.jobs.sync_calendar
///   {prefix}.jobs.schedule_meeting
use planpal::{logging, queue::{jobs, Job}, worker};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    logging::init_from_env();
    logging::info("Starting PlanPal schedule-worker");

    let state = worker::bootstrap().await;

    worker::run(
        "schedule-worker",
        state,
        &["sync_calendar", "schedule_meeting"],
        |state, job| Box::pin(dispatch(state, job)),
    )
    .await;
}

async fn dispatch(state: planpal::AppState, job: Job) -> Result<(), planpal::error::AppError> {
    match job {
        Job::SyncCalendar { provider_id } => {
            jobs::sync_calendar::run(provider_id, &state).await
        }
        Job::ScheduleMeeting { meeting_request_id } => {
            jobs::schedule_meeting::run(meeting_request_id, &state).await
        }
        other => {
            planpal::logging::warn_with(
                &[("job", &format!("{other:?}"))],
                "schedule-worker received unhandled job type — skipping",
            );
            Ok(())
        }
    }
}
