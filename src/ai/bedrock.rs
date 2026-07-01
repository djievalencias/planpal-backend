use async_trait::async_trait;
use aws_sdk_bedrockruntime::error::ProvideErrorMetadata;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, Message as BedrockMessage, SystemContentBlock,
};
use aws_sdk_bedrockruntime::Client as BedrockClient;

use crate::error::AppError;
use crate::logging;
use crate::model::ai_chat::{AiChatMessage, AiChatRole};

use super::prompt::AiResponse;
use super::AiProvider;

pub struct BedrockProvider {
    client: BedrockClient,
    model_id: String,
}

impl BedrockProvider {
    pub fn new(client: BedrockClient, model_id: String) -> Self {
        Self { client, model_id }
    }
}

pub async fn build_client(region: &str) -> BedrockClient {
    let aws_cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;
    BedrockClient::new(&aws_cfg)
}

#[async_trait]
impl AiProvider for BedrockProvider {
    async fn converse(
        &self,
        system_prompt: &str,
        history: &[AiChatMessage],
    ) -> Result<AiResponse, AppError> {
        let system = vec![SystemContentBlock::Text(system_prompt.to_string())];

        let messages: Vec<BedrockMessage> = history
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    AiChatRole::User => ConversationRole::User,
                    AiChatRole::Assistant => ConversationRole::Assistant,
                };
                BedrockMessage::builder()
                    .role(role)
                    .content(ContentBlock::Text(msg.content.clone()))
                    .build()
                    .expect("message build")
            })
            .collect();

        logging::info_with(
            &[
                ("provider", "bedrock"),
                ("model_id", &self.model_id),
                ("message_count", &messages.len().to_string()),
            ],
            "ai: calling model",
        );

        let start = std::time::Instant::now();

        let resp = self
            .client
            .converse()
            .model_id(&self.model_id)
            .set_system(Some(system))
            .set_messages(Some(messages))
            .send()
            .await
            .map_err(|e| {
                let meta = e.meta();
                let code = meta.code().unwrap_or("unknown");
                let message = meta.message().unwrap_or("no details");
                logging::error_with(
                    &[
                        ("provider", "bedrock"),
                        ("error", &e.to_string()),
                        ("code", code),
                        ("message", message),
                        ("model_id", &self.model_id),
                        ("elapsed_ms", &start.elapsed().as_millis().to_string()),
                    ],
                    "ai: model call failed",
                );
                AppError::Internal(
                    "I'm having trouble processing your request right now. Please try again in a moment.".into(),
                )
            })?;

        let elapsed_ms = start.elapsed().as_millis();
        let input_tokens = resp.usage().map_or("-".to_string(), |u| u.input_tokens().to_string());
        let output_tokens = resp.usage().map_or("-".to_string(), |u| u.output_tokens().to_string());
        let stop_reason = format!("{:?}", resp.stop_reason());

        let output_text = resp
            .output()
            .and_then(|o| o.as_message().ok())
            .and_then(|m| m.content().first())
            .and_then(|c| c.as_text().ok())
            .ok_or_else(|| {
                logging::error_with(
                    &[("provider", "bedrock"), ("model_id", &self.model_id), ("stop_reason", &stop_reason)],
                    "ai: empty response from model",
                );
                AppError::Internal(
                    "I'm having trouble processing your request right now. Please try again in a moment.".into(),
                )
            })?
            .to_string();

        logging::info_with(
            &[
                ("provider", "bedrock"),
                ("model_id", &self.model_id),
                ("elapsed_ms", &elapsed_ms.to_string()),
                ("input_tokens", &input_tokens),
                ("output_tokens", &output_tokens),
                ("stop_reason", &stop_reason),
            ],
            "ai: model responded",
        );

        super::prompt::parse_ai_response(&output_text)
    }
}
