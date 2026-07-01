/// AI worker — processes chat-based meeting scheduling via AI.
///
/// Supports multiple AI providers (Bedrock, Anthropic API) configured via
/// `ai.provider` in config / secret manager.
///
/// Subscribed subjects:
///   {prefix}.jobs.process_ai_chat
use planpal::{ai, logging, queue::{jobs, Job}, worker};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    logging::init_from_env();
    logging::info("Starting PlanPal ai-worker");

    let mut state = worker::bootstrap().await;

    // Initialize AI provider based on config
    let provider = ai::build_provider(&state.config.ai).await;
    state.ai_provider = Some(provider);

    worker::run(
        "ai-worker",
        state,
        &["process_ai_chat"],
        |state, job| Box::pin(dispatch(state, job)),
    )
    .await;
}

async fn dispatch(state: planpal::AppState, job: Job) -> Result<(), planpal::error::AppError> {
    match job {
        Job::ProcessAiChat { session_id, message_id } => {
            jobs::process_ai_chat::run(session_id, message_id, &state).await
        }
        other => {
            planpal::logging::warn_with(
                &[("job", &format!("{other:?}"))],
                "ai-worker received unhandled job type — skipping",
            );
            Ok(())
        }
    }
}
