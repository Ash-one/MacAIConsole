//! MockProvider — 文档 §62 的第一开发任务核心。
//!
//! 行为：回显最后一条 user 消息；SSE 按 2 字符分块 + 25ms 间隔，
//! 真实模拟 token streaming 以便验证 Provider → Rust stream → SSE 链路。

use std::time::Duration;

use async_trait::async_trait;
use tokio::time::sleep;

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, ChatProvider, ChatStream, IsolationMode, ModelHandle, Provider, ProviderDescriptor,
    ProviderError, ProviderHealth, ProviderStatus,
};
use ai_core::request::ChatRequest;
use ai_core::response::{ChatChunk, ChatChunkChoice, ChatChunkDelta, ChatResponse, ChatUsage};

pub struct MockProvider;

impl MockProvider {
    /// 取最后一条 user 消息作为回复（echo）。
    fn reply_text(request: &ChatRequest) -> String {
        request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_else(|| "Hello from mock provider".to_string())
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::Chat]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::InProcess,
            supported_devices: vec!["cpu".to_string()],
        }
    }

    async fn status(&self) -> ProviderStatus {
        ProviderStatus {
            available: true,
            ready: true,
            effective_device: Some("cpu".to_string()),
            resident_models: vec![],
            reason: None,
            install_hint: None,
        }
    }

    async fn load(&self, _model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        Ok(ModelHandle {
            model_id: "mock".to_string(),
            provider_id: "mock".to_string(),
        })
    }

    async fn unload(&self, _handle: &ModelHandle) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth {
            ok: true,
            message: Some("mock provider healthy".to_string()),
        })
    }
}

#[async_trait]
impl ChatProvider for MockProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let reply = Self::reply_text(&request);
        let prompt_tokens: u64 = request
            .messages
            .iter()
            .map(|m| m.content.len() as u64)
            .sum();
        let usage = ChatUsage {
            prompt_tokens,
            completion_tokens: reply.len() as u64,
            total_tokens: prompt_tokens + reply.len() as u64,
        };
        Ok(ChatResponse {
            id: format!("chatcmpl-mock-{}", now_id()),
            object: "chat.completion".to_string(),
            created: now_unix(),
            model: request.model.clone(),
            choices: vec![ai_core::response::ChatChoice {
                index: 0,
                message: ai_core::response::ChatResponseMessage {
                    role: "assistant".to_string(),
                    content: reply,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage,
        })
    }

    /// 流式版本：reply 按 2 字符分块，最后发 finish chunk。
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        let reply = Self::reply_text(&request);
        let model = request.model.clone();
        let id = format!("chatcmpl-mock-{}", now_id());
        let created = now_unix();

        // 分块：2 字符一块。
        let chars: Vec<String> = reply
            .chars()
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|c| c.iter().collect())
            .collect();

        // unfold 状态：(已发出的块数 i, 是否已发 finish chunk)。
        // 状态机：i < len → content chunk；i == len 且 !finished → finish chunk；
        // finished → None 终止流。
        let stream = futures::stream::unfold(
            (chars, 0usize, false, id, created, model),
            |(chars, i, finished, id, created, model)| async move {
                if finished {
                    return None;
                }
                sleep(Duration::from_millis(25)).await;
                if i < chars.len() {
                    let chunk = ChatChunk {
                        id: id.clone(),
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model: model.clone(),
                        choices: vec![ChatChunkChoice {
                            index: 0,
                            delta: ChatChunkDelta {
                                role: if i == 0 {
                                    Some("assistant".to_string())
                                } else {
                                    None
                                },
                                content: Some(chars[i].clone()),
                            },
                            finish_reason: None,
                        }],
                    };
                    Some((Ok(chunk), (chars, i + 1, false, id, created, model)))
                } else {
                    let chunk = ChatChunk {
                        id,
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model,
                        choices: vec![ChatChunkChoice {
                            index: 0,
                            delta: ChatChunkDelta::default(),
                            finish_reason: Some("stop".to_string()),
                        }],
                    };
                    // 下一轮进入 finished=true → 终止。
                    Some((
                        Ok(chunk),
                        (chars, i, true, String::new(), created, String::new()),
                    ))
                }
            },
        );

        Ok(Box::pin(stream))
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_id() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::request::ChatMessage;
    use futures::StreamExt;

    fn request(content: &str) -> ChatRequest {
        ChatRequest {
            model: "mock".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: content.to_string(),
            }],
            ..ChatRequest::default()
        }
    }

    #[tokio::test]
    async fn echoes_last_user_message() {
        let response = MockProvider.chat(request("hello")).await.unwrap();

        assert_eq!(response.model, "mock");
        assert_eq!(response.choices[0].message.content, "hello");
        assert_eq!(response.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn stream_reconstructs_echo_and_finishes() {
        let chunks: Vec<_> = MockProvider
            .chat_stream(request("你好，world"))
            .await
            .unwrap()
            .collect()
            .await;

        let content: String = chunks
            .iter()
            .filter_map(|chunk| chunk.as_ref().unwrap().choices[0].delta.content.as_deref())
            .collect();

        assert_eq!(content, "你好，world");
        assert_eq!(
            chunks[0].as_ref().unwrap().choices[0].delta.role.as_deref(),
            Some("assistant")
        );
        assert_eq!(
            chunks.last().unwrap().as_ref().unwrap().choices[0]
                .finish_reason
                .as_deref(),
            Some("stop")
        );
    }
}
