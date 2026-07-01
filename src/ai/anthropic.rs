use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::logging;
use crate::model::ai_chat::{AiChatMessage, AiChatRole};

use super::prompt::AiResponse;
use super::AiProvider;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;

pub struct AnthropicProvider {
    model_id: String,
    api_key: String,
    http: Client,
}

impl AnthropicProvider {
    pub fn new(model_id: String, api_key: String) -> Self {
        Self {
            model_id,
            api_key,
            http: Client::new(),
        }
    }
}

// ── Anthropic Messages API request/response shapes ──────────────────────────

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<MessageEntry<'a>>,
}

#[derive(Serialize)]
struct MessageEntry<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    usage: Option<Usage>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: Option<String>,
    #[serde(rename = "type")]
    error_type: Option<String>,
}

// ── AiProvider implementation ────────────────────────────────────────────────

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn converse(
        &self,
        system_prompt: &str,
        history: &[AiChatMessage],
    ) -> Result<AiResponse, AppError> {
        let messages: Vec<MessageEntry> = history
            .iter()
            .map(|msg| MessageEntry {
                role: match msg.role {
                    AiChatRole::User => "user",
                    AiChatRole::Assistant => "assistant",
                },
                content: &msg.content,
            })
            .collect();

        let body = MessagesRequest {
            model: &self.model_id,
            max_tokens: MAX_TOKENS,
            system: system_prompt,
            messages,
        };

        logging::info_with(
            &[
                ("provider", "anthropic"),
                ("model_id", &self.model_id),
                ("message_count", &history.len().to_string()),
            ],
            "ai: calling model",
        );

        let start = std::time::Instant::now();

        let resp = self
            .http
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                logging::error_with(
                    &[
                        ("provider", "anthropic"),
                        ("error", &e.to_string()),
                        ("elapsed_ms", &start.elapsed().as_millis().to_string()),
                    ],
                    "ai: HTTP request failed",
                );
                AppError::Internal(
                    "I'm having trouble processing your request right now. Please try again in a moment.".into(),
                )
            })?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            let error_msg = serde_json::from_str::<ErrorResponse>(&error_body)
                .ok()
                .and_then(|e| e.error)
                .map(|e| {
                    format!(
                        "type={} message={}",
                        e.error_type.as_deref().unwrap_or("unknown"),
                        e.message.as_deref().unwrap_or("no details"),
                    )
                })
                .unwrap_or_else(|| error_body.clone());

            logging::error_with(
                &[
                    ("provider", "anthropic"),
                    ("status", &status.as_u16().to_string()),
                    ("error", &error_msg),
                    ("model_id", &self.model_id),
                    ("elapsed_ms", &start.elapsed().as_millis().to_string()),
                ],
                "ai: model call failed",
            );
            return Err(AppError::Internal(
                "I'm having trouble processing your request right now. Please try again in a moment.".into(),
            ));
        }

        let elapsed_ms = start.elapsed().as_millis();
        let response: MessagesResponse = resp.json().await.map_err(|e| {
            logging::error_with(
                &[("provider", "anthropic"), ("error", &e.to_string())],
                "ai: failed to parse API response",
            );
            AppError::Internal(
                "I'm having trouble processing your request right now. Please try again in a moment.".into(),
            )
        })?;

        let input_tokens = response.usage.as_ref().and_then(|u| u.input_tokens).map_or("-".into(), |t| t.to_string());
        let output_tokens = response.usage.as_ref().and_then(|u| u.output_tokens).map_or("-".into(), |t| t.to_string());
        let stop_reason = response.stop_reason.as_deref().unwrap_or("-");

        let output_text = response
            .content
            .first()
            .and_then(|c| c.text.as_deref())
            .ok_or_else(|| {
                logging::error_with(
                    &[("provider", "anthropic"), ("model_id", &self.model_id)],
                    "ai: empty response from model",
                );
                AppError::Internal(
                    "I'm having trouble processing your request right now. Please try again in a moment.".into(),
                )
            })?;

        logging::info_with(
            &[
                ("provider", "anthropic"),
                ("model_id", &self.model_id),
                ("elapsed_ms", &elapsed_ms.to_string()),
                ("input_tokens", &input_tokens),
                ("output_tokens", &output_tokens),
                ("stop_reason", stop_reason),
            ],
            "ai: model responded",
        );

        super::prompt::parse_ai_response(output_text)
    }
}
