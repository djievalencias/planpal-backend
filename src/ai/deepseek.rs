/// DeepSeek provider — uses the OpenAI-compatible chat completions API.
///
/// Also works with any OpenAI-compatible endpoint (Groq, Together, etc.)
/// by changing the base URL via `ai.api_base_url` config.
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::logging;
use crate::model::ai_chat::{AiChatMessage, AiChatRole};

use super::prompt::AiResponse;
use super::AiProvider;

const DEEPSEEK_API_URL: &str = "https://api.deepseek.com/v1/chat/completions";
const MAX_TOKENS: u32 = 1024;

pub struct DeepSeekProvider {
    model_id: String,
    api_key: String,
    api_url: String,
    http: Client,
}

impl DeepSeekProvider {
    pub fn new(model_id: String, api_key: String, api_url: Option<String>) -> Self {
        Self {
            model_id,
            api_key,
            api_url: api_url.unwrap_or_else(|| DEEPSEEK_API_URL.to_string()),
            http: Client::new(),
        }
    }
}

// ── OpenAI-compatible request/response shapes ───────────────────────────────

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
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
impl AiProvider for DeepSeekProvider {
    async fn converse(
        &self,
        system_prompt: &str,
        history: &[AiChatMessage],
    ) -> Result<AiResponse, AppError> {
        let mut messages = vec![ChatMessage {
            role: "system",
            content: system_prompt,
        }];

        for msg in history {
            messages.push(ChatMessage {
                role: match msg.role {
                    AiChatRole::User => "user",
                    AiChatRole::Assistant => "assistant",
                },
                content: &msg.content,
            });
        }

        let body = ChatRequest {
            model: &self.model_id,
            max_tokens: MAX_TOKENS,
            messages,
        };

        logging::info_with(
            &[
                ("provider", "deepseek"),
                ("model_id", &self.model_id),
                ("message_count", &history.len().to_string()),
            ],
            "ai: calling model",
        );

        let start = std::time::Instant::now();

        let resp = self
            .http
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                logging::error_with(
                    &[
                        ("provider", "deepseek"),
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
                    ("provider", "deepseek"),
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
        let response: ChatResponse = resp.json().await.map_err(|e| {
            logging::error_with(
                &[("provider", "deepseek"), ("error", &e.to_string())],
                "ai: failed to parse API response",
            );
            AppError::Internal(
                "I'm having trouble processing your request right now. Please try again in a moment.".into(),
            )
        })?;

        let input_tokens = response.usage.as_ref().and_then(|u| u.prompt_tokens).map_or("-".into(), |t| t.to_string());
        let output_tokens = response.usage.as_ref().and_then(|u| u.completion_tokens).map_or("-".into(), |t| t.to_string());
        let finish_reason = response.choices.first().and_then(|c| c.finish_reason.as_deref()).unwrap_or("-");

        let output_text = response
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .ok_or_else(|| {
                logging::error_with(
                    &[("provider", "deepseek"), ("model_id", &self.model_id)],
                    "ai: empty response from model",
                );
                AppError::Internal(
                    "I'm having trouble processing your request right now. Please try again in a moment.".into(),
                )
            })?;

        logging::info_with(
            &[
                ("provider", "deepseek"),
                ("model_id", &self.model_id),
                ("elapsed_ms", &elapsed_ms.to_string()),
                ("input_tokens", &input_tokens),
                ("output_tokens", &output_tokens),
                ("finish_reason", finish_reason),
            ],
            "ai: model responded",
        );

        super::prompt::parse_ai_response(output_text)
    }
}
