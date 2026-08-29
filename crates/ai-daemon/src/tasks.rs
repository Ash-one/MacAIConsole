//! 有界的内存任务历史。
//!
//! 任务注册表同时是任务列表和 `RuntimeInfo.active_requests` 的唯一事实来源。
//! 运行中任务不淘汰，终态任务只保留最近 100 条；任务句柄未显式结束时，
//! 其 Drop 语义表示客户端断开或请求被取消。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use ai_core::request::{ChatRequest, SpeechRequest, TranscriptionRequest};
use ai_core::response::{
    TaskDetail, TaskListResponse, TaskRequestDetail, TaskResultDetail, TaskSummary,
};

pub const MAX_COMPLETED_TASKS: usize = 100;
pub const TASK_TEXT_LIMIT: usize = 64 * 1024;

/// 终态任务的错误文本同样受有界内存约束：超出上限时做 UTF-8 安全截断并追加标记，
/// 避免大段 provider stderr 撑爆有界历史记录、膨胀 /api/tasks 响应。
const ERROR_TRUNCATION_MARKER: &str = "…（已截断）";
const ERROR_TEXT_LIMIT: usize = TASK_TEXT_LIMIT.saturating_sub(ERROR_TRUNCATION_MARKER.len());

#[derive(Clone)]
pub struct TaskRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

struct RegistryState {
    running: HashMap<String, TaskDetail>,
    completed: VecDeque<TaskDetail>,
}

/// 一个请求持有一个句柄。句柄 Drop 前若没有显式成功/失败，任务会进入 cancelled。
pub struct TaskHandle {
    registry: TaskRegistry,
    id: String,
    ended: AtomicBool,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryState {
                running: HashMap::new(),
                completed: VecDeque::new(),
            })),
        }
    }

    pub fn start(
        &self,
        id: impl Into<String>,
        kind: impl Into<String>,
        model: impl Into<String>,
        request: TaskRequestDetail,
    ) -> TaskHandle {
        let id = id.into();
        let kind = kind.into();
        let model = model.into();
        let (request, request_truncated) = normalize_request(request);
        let detail = TaskDetail {
            id: id.clone(),
            kind,
            status: "running".to_string(),
            model,
            provider: None,
            started_at_ms: unix_now_ms(),
            completed_at_ms: None,
            duration_ms: None,
            request,
            result: TaskResultDetail::default(),
            request_truncated,
            result_truncated: false,
            error: None,
        };
        self.inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .insert(id.clone(), detail);
        TaskHandle {
            registry: self.clone(),
            id,
            ended: AtomicBool::new(false),
        }
    }

    pub fn running_count(&self) -> u64 {
        self.inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .len() as u64
    }

    pub fn list(&self, completed_limit: usize) -> TaskListResponse {
        let limit = completed_limit.clamp(1, MAX_COMPLETED_TASKS);
        let state = self.inner.lock().expect("task registry lock poisoned");
        let mut running: Vec<TaskSummary> =
            state.running.values().map(TaskDetail::summary).collect();
        running.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        let completed = state
            .completed
            .iter()
            .take(limit)
            .map(TaskDetail::summary)
            .collect();
        TaskListResponse { running, completed }
    }

    pub fn get(&self, id: &str) -> Option<TaskDetail> {
        let state = self.inner.lock().expect("task registry lock poisoned");
        state
            .running
            .get(id)
            .or_else(|| state.completed.iter().find(|detail| detail.id == id))
            .cloned()
    }

    #[cfg(test)]
    fn completed_len(&self) -> usize {
        self.inner
            .lock()
            .expect("task registry lock poisoned")
            .completed
            .len()
    }

    fn set_provider(&self, id: &str, provider: Option<String>) {
        if let Some(detail) = self
            .inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .get_mut(id)
        {
            detail.provider = provider;
        }
    }

    fn update_request<F>(&self, id: &str, update: F)
    where
        F: FnOnce(&mut TaskRequestDetail),
    {
        if let Some(detail) = self
            .inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .get_mut(id)
        {
            update(&mut detail.request);
            let (request, truncated) = normalize_request(detail.request.clone());
            detail.request = request;
            detail.request_truncated |= truncated;
        }
    }

    fn append_output(&self, id: &str, output: &str) {
        if output.is_empty() {
            return;
        }
        if let Some(detail) = self
            .inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .get_mut(id)
        {
            let current = detail.result.output_text.take().unwrap_or_default();
            let available = TASK_TEXT_LIMIT.saturating_sub(current.len());
            let (suffix, truncated) = take_prefix(output, available);
            detail.result.output_text = Some(current + &suffix);
            detail.result_truncated |= truncated;
        }
    }

    fn update_result<F>(&self, id: &str, update: F)
    where
        F: FnOnce(&mut TaskResultDetail),
    {
        if let Some(detail) = self
            .inner
            .lock()
            .expect("task registry lock poisoned")
            .running
            .get_mut(id)
        {
            update(&mut detail.result);
            let (result, truncated) = normalize_result(detail.result.clone());
            detail.result = result;
            detail.result_truncated |= truncated;
        }
    }

    fn finalize(
        &self,
        id: &str,
        status: &str,
        result: Option<TaskResultDetail>,
        error: Option<String>,
    ) {
        let mut state = self.inner.lock().expect("task registry lock poisoned");
        let Some(mut detail) = state.running.remove(id) else {
            return;
        };
        if let Some(result) = result {
            let (result, truncated) = normalize_result(result);
            detail.result = result;
            detail.result_truncated |= truncated;
        }
        detail.status = status.to_string();
        detail.completed_at_ms = Some(unix_now_ms());
        detail.duration_ms = detail
            .completed_at_ms
            .map(|completed| completed.saturating_sub(detail.started_at_ms));
        detail.error = error.map(|error| {
            let (error, truncated) = take_prefix(&error, ERROR_TEXT_LIMIT);
            if truncated {
                format!("{error}{ERROR_TRUNCATION_MARKER}")
            } else {
                error
            }
        });
        state.completed.push_front(detail);
        while state.completed.len() > MAX_COMPLETED_TASKS {
            state.completed.pop_back();
        }
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn set_provider(&self, provider: Option<String>) {
        self.registry.set_provider(&self.id, provider);
    }

    pub fn update_request<F>(&self, update: F)
    where
        F: FnOnce(&mut TaskRequestDetail),
    {
        self.registry.update_request(&self.id, update);
    }

    pub fn append_output(&self, output: &str) {
        self.registry.append_output(&self.id, output);
    }

    pub fn set_finish_reason(&self, finish_reason: Option<String>) {
        self.registry.update_result(&self.id, |result| {
            result.finish_reason = finish_reason;
        });
    }

    pub fn succeed(&self, result: TaskResultDetail) {
        self.try_finalize("succeeded", Some(result), None);
    }

    pub fn succeed_current(&self) {
        self.try_finalize("succeeded", None, None);
    }

    pub fn fail(&self, message: impl Into<String>) {
        self.try_finalize("failed", None, Some(message.into()));
    }

    fn try_finalize(&self, status: &str, result: Option<TaskResultDetail>, error: Option<String>) {
        if self
            .ended
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.registry.finalize(&self.id, status, result, error);
        }
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if self
            .ended
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.registry.finalize(
                &self.id,
                "cancelled",
                None,
                Some("客户端连接已中断".to_string()),
            );
        }
    }
}

pub fn chat_request_detail(request: &ChatRequest) -> TaskRequestDetail {
    TaskRequestDetail {
        messages: request.messages.clone(),
        stream: Some(request.stream),
        temperature: request.temperature,
        max_tokens: request.max_tokens,
        ..TaskRequestDetail::default()
    }
}

pub fn transcription_request_detail(
    request: &TranscriptionRequest,
    file_name: Option<String>,
    file_size_bytes: Option<u64>,
    audio_duration_ms: Option<u64>,
) -> TaskRequestDetail {
    TaskRequestDetail {
        file_name,
        file_size_bytes,
        audio_duration_ms,
        language: request.language.clone(),
        format: request.response_format.clone(),
        ..TaskRequestDetail::default()
    }
}

pub fn speech_request_detail(request: &SpeechRequest) -> TaskRequestDetail {
    TaskRequestDetail {
        input_text: Some(request.input.clone()),
        voice: request.voice.clone(),
        format: request.format.clone(),
        speed: request.speed,
        ..TaskRequestDetail::default()
    }
}

fn normalize_request(mut request: TaskRequestDetail) -> (TaskRequestDetail, bool) {
    let mut remaining = TASK_TEXT_LIMIT;
    let mut truncated = false;
    for message in &mut request.messages {
        let (content, did_truncate) = take_prefix(&message.content, remaining);
        message.content = content;
        remaining = remaining.saturating_sub(message.content.len());
        truncated |= did_truncate;
    }
    if let Some(input_text) = request.input_text.take() {
        let (input_text, did_truncate) = take_prefix(&input_text, remaining);
        request.input_text = Some(input_text);
        truncated |= did_truncate;
    }
    (request, truncated)
}

fn normalize_result(mut result: TaskResultDetail) -> (TaskResultDetail, bool) {
    let Some(output_text) = result.output_text.take() else {
        return (result, false);
    };
    let (output_text, truncated) = take_prefix(&output_text, TASK_TEXT_LIMIT);
    result.output_text = Some(output_text);
    (result, truncated)
}

fn take_prefix(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::request::ChatMessage;

    fn request(text: &str) -> TaskRequestDetail {
        TaskRequestDetail {
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
            }],
            ..TaskRequestDetail::default()
        }
    }

    fn result(text: &str) -> TaskResultDetail {
        TaskResultDetail {
            output_text: Some(text.to_string()),
            ..TaskResultDetail::default()
        }
    }

    #[test]
    fn new_task_enters_running_and_success_moves_to_completed() {
        let registry = TaskRegistry::new();
        let handle = registry.start("task-1", "chat", "mock", request("hello"));
        assert_eq!(registry.running_count(), 1);
        assert_eq!(registry.get("task-1").unwrap().status, "running");

        handle.succeed(result("world"));

        assert_eq!(registry.running_count(), 0);
        let detail = registry.get("task-1").unwrap();
        assert_eq!(detail.status, "succeeded");
        assert_eq!(detail.result.output_text.as_deref(), Some("world"));
        assert!(detail.duration_ms.is_some());
    }

    #[test]
    fn stt_audio_duration_reaches_task_summary() {
        let registry = TaskRegistry::new();
        let request = TranscriptionRequest {
            model: "whisper-large-v3-turbo".to_string(),
            file: Some("meeting.wav".to_string()),
            language: Some("zh".to_string()),
            response_format: Some("json".to_string()),
        };
        let detail = transcription_request_detail(
            &request,
            Some("meeting.wav".to_string()),
            Some(502_444),
            Some(15_700),
        );
        let _handle = registry.start("task-stt", "stt", request.model, detail);

        let list = registry.list(100);
        assert_eq!(list.running[0].audio_duration_ms, Some(15_700));
        assert_eq!(
            registry.get("task-stt").unwrap().request.audio_duration_ms,
            Some(15_700)
        );
    }

    #[test]
    fn failure_preserves_error() {
        let registry = TaskRegistry::new();
        let handle = registry.start("task-1", "chat", "missing", request("hello"));
        handle.fail("model not found");

        let detail = registry.get("task-1").unwrap();
        assert_eq!(detail.status, "failed");
        assert_eq!(detail.error.as_deref(), Some("model not found"));
    }

    #[test]
    fn dropping_an_unfinished_handle_marks_cancelled() {
        let registry = TaskRegistry::new();
        {
            let handle = registry.start("task-1", "chat", "mock", request("hello"));
            assert_eq!(handle.id(), "task-1");
        }
        let detail = registry.get("task-1").unwrap();
        assert_eq!(detail.status, "cancelled");
        assert_eq!(detail.error.as_deref(), Some("客户端连接已中断"));
    }

    #[test]
    fn completed_history_is_bounded_but_running_tasks_are_not_evicted() {
        let registry = TaskRegistry::new();
        let running = registry.start("running", "chat", "mock", request("still running"));
        for index in 0..=MAX_COMPLETED_TASKS {
            let handle = registry.start(format!("task-{index}"), "chat", "mock", request("done"));
            handle.succeed(result("ok"));
        }

        assert_eq!(registry.completed_len(), MAX_COMPLETED_TASKS);
        assert!(registry.get("task-0").is_none());
        assert_eq!(registry.running_count(), 1);
        assert_eq!(registry.get(running.id()).unwrap().status, "running");
        drop(running);
    }

    #[test]
    fn request_and_result_text_are_capped_at_64_kib() {
        let registry = TaskRegistry::new();
        let long = "x".repeat(TASK_TEXT_LIMIT + 32);
        let handle = registry.start("task-1", "chat", "mock", request(&long));
        handle.succeed(result(&long));

        let detail = registry.get("task-1").unwrap();
        assert!(detail.request_truncated);
        assert!(detail.result_truncated);
        assert_eq!(detail.request.messages[0].content.len(), TASK_TEXT_LIMIT);
        assert_eq!(detail.result.output_text.unwrap().len(), TASK_TEXT_LIMIT);
    }

    #[test]
    fn failed_task_error_text_is_utf8_safely_capped() {
        let registry = TaskRegistry::new();
        // 全部由 3 字节字符组成，总长度超过上限；截断必然落在字符边界（否则切片会 panic）。
        let long = "字".repeat(TASK_TEXT_LIMIT / 3 + 10);
        let handle = registry.start("task-1", "chat", "mock", request("hello"));
        handle.fail(long.as_str());

        let detail = registry.get("task-1").unwrap();
        assert_eq!(detail.status, "failed");
        let error = detail.error.unwrap();
        assert!(error.len() <= TASK_TEXT_LIMIT);
        assert!(error.ends_with("…（已截断）"));
        // 截断发生在字符边界：去标记后的剩余部分必须是原文本的前缀。
        let prefix = error.trim_end_matches("…（已截断）");
        assert!(long.starts_with(prefix));
    }

    #[test]
    fn list_clamps_completed_limit_and_keeps_newest_first() {
        let registry = TaskRegistry::new();
        for index in 0..3 {
            let handle = registry.start(format!("task-{index}"), "chat", "mock", request("done"));
            handle.succeed(result("ok"));
        }
        let list = registry.list(0);
        assert_eq!(list.completed.len(), 1);
        assert_eq!(list.completed[0].id, "task-2");
        assert_eq!(registry.list(1000).completed.len(), 3);
    }
}
