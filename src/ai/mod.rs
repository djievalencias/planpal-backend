pub mod anthropic;
pub mod bedrock;
pub mod deepseek;
pub mod prompt;

use crate::config::AiConfig;
use crate::error::AppError;
use crate::model::ai_chat::AiChatMessage;
use async_trait::async_trait;
use prompt::AiResponse;
use std::sync::Arc;

/// Trait for AI providers — implemented by Bedrock, Anthropic API, etc.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Send a conversation to the AI and get a structured response.
    async fn converse(
        &self,
        system_prompt: &str,
        history: &[AiChatMessage],
    ) -> Result<AiResponse, AppError>;
}

/// Build the configured AI provider based on `ai.provider` config value.
///
/// Supported providers:
/// - `"bedrock"` (default) — AWS Bedrock Converse API
/// - `"anthropic"` — Anthropic Messages API (api.anthropic.com)
/// - `"deepseek"` — DeepSeek chat API (OpenAI-compatible)
pub async fn build_provider(cfg: &AiConfig) -> Arc<dyn AiProvider> {
    match cfg.provider.to_lowercase().as_str() {
        "anthropic" => {
            crate::logging::info_with(
                &[("provider", "anthropic"), ("model_id", &cfg.model_id)],
                "AI provider: Anthropic API",
            );
            Arc::new(anthropic::AnthropicProvider::new(
                cfg.model_id.clone(),
                cfg.api_key.clone(),
            ))
        }
        "deepseek" => {
            let base_url = if cfg.api_base_url.is_empty() {
                None
            } else {
                Some(cfg.api_base_url.clone())
            };
            crate::logging::info_with(
                &[
                    ("provider", "deepseek"),
                    ("model_id", &cfg.model_id),
                    ("api_base_url", base_url.as_deref().unwrap_or("(default)")),
                ],
                "AI provider: DeepSeek API",
            );
            Arc::new(deepseek::DeepSeekProvider::new(
                cfg.model_id.clone(),
                cfg.api_key.clone(),
                base_url,
            ))
        }
        _ => {
            crate::logging::info_with(
                &[("provider", "bedrock"), ("model_id", &cfg.model_id), ("region", &cfg.region)],
                "AI provider: AWS Bedrock",
            );
            let client = bedrock::build_client(&cfg.region).await;
            Arc::new(bedrock::BedrockProvider::new(client, cfg.model_id.clone()))
        }
    }
}
