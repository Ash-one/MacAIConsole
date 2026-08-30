//! Runtime — daemon 唯一的模型与 Provider Authority。
//!
//! Milestone 1 使用内存模型注册表；Provider 已支持 Mock 与隔离的 llama.cpp
//! worker。SQLite、内存预算和 LRU 在后续 milestone 接入，不改变这里的调用边界。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, RwLock};

use ai_core::model::ModelSpec;
use ai_core::provider::{
    ChatProvider, ModelHandle, Provider, ProviderDescriptor, ProviderError, ProviderStatus,
    STTProvider, TTSProvider,
};
use ai_core::request::{SpeechRequest, TranscriptionRequest};
use ai_core::response::{
    LoadedModelInfo, RuntimeInfo, SpeechResponse, TaskRequestDetail, TaskResultDetail,
    TranscriptionResponse,
};
use ai_core::AIError;

use crate::providers::{
    KokoroMlxProvider, LlamaCppProvider, MacOSSayProvider, MlxLmProvider, MockProvider,
    Qwen3AsrProvider, WhisperCppProvider,
};
use crate::registry::RegistryStore;
use crate::scheduler;
use crate::tasks::{TaskHandle, TaskRegistry};

/// 内存版模型注册表条目：模型规格 + 当前状态 + 使用时间。
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub spec: ModelSpec,
    pub state: String,
    pub loaded_at: Option<u64>,
    pub last_used_at: Option<u64>,
}

pub struct Runtime {
    registry: RwLock<HashMap<String, RegistryEntry>>,
    /// SQLite 持久层。None = 纯内存模式（单元测试）。
    store: Option<RegistryStore>,
    /// AI 可用内存预算（字节）；None 表示无法探测，跳过预算约束。
    memory_budget: Option<u64>,
    providers: HashMap<String, Arc<dyn Provider>>,
    chat_providers: HashMap<String, Arc<dyn ChatProvider>>,
    stt_providers: HashMap<String, Arc<dyn STTProvider>>,
    tts_providers: HashMap<String, Arc<dyn TTSProvider>>,
    handles: RwLock<HashMap<String, ModelHandle>>,
    model_leases: StdMutex<HashMap<String, u64>>,
    lifecycle_lock: Mutex<()>,
    started_at: Instant,
    tasks: TaskRegistry,
    request_counter: AtomicU64,
}

pub struct ModelLease {
    runtime: Arc<Runtime>,
    model_id: String,
}

impl Drop for ModelLease {
    fn drop(&mut self) {
        if let Ok(mut leases) = self.runtime.model_leases.lock() {
            match leases.get_mut(&self.model_id) {
                Some(count) if *count > 1 => *count -= 1,
                Some(_) => {
                    leases.remove(&self.model_id);
                }
                None => {}
            }
        }
    }
}

impl Runtime {
    /// 纯内存构造，仅供单元测试：额外注入 mock 与 macos-say 测试 provider。
    pub fn new() -> Self {
        let mut runtime = Self::with_options_and_seed(None, Vec::new());
        runtime.inject_test_providers();
        runtime
    }

    /// 注入测试用 provider（mock 回显、macos-say 系统 TTS）。
    /// 它们不进入生产 providers 表——管理面（/api/providers、/v1/models）
    /// 只展示真实引擎；macos-say 仅作为 `macai speak` 未指定模型时的兜底能力。
    fn inject_test_providers(&mut self) {
        let mock = Arc::new(MockProvider);
        self.providers.insert("mock".to_string(), mock.clone());
        self.chat_providers.insert("mock".to_string(), mock);
        let say = Arc::new(MacOSSayProvider::new());
        self.providers.insert("macos-say".to_string(), say.clone());
        self.tts_providers.insert("macos-say".to_string(), say);
    }

    /// 生产构造：打开 SQLite 注册表并加载已注册模型；预算由 scheduler 计算。
    /// 数据库打开失败时降级为内存模式并记录告警，daemon 仍可运行。
    pub fn with_store(db_path: &std::path::Path) -> Self {
        let store = match RegistryStore::open(db_path) {
            Ok(store) => Some(store),
            Err(error) => {
                tracing::warn!(%error, "registry db unavailable, falling back to in-memory");
                None
            }
        };
        // 先在锁外把持久层读成普通 Vec，再在构造时注入初始 registry，
        // 避免在 async 上下文里做阻塞锁操作。
        let restored: Vec<(ModelSpec, Option<u64>)> = store
            .as_ref()
            .map(|store| store.load_all())
            .unwrap_or_default();
        let runtime = Self::with_options_and_seed(store, restored.clone());
        if runtime.store.is_some() {
            // 在锁外已算好的种子条目数，避免 async 上下文里做阻塞读（会 panic）。
            let count = restored.len();
            tracing::info!(count, "model registry restored from sqlite");
        }
        runtime
    }

    fn with_options_and_seed(
        store: Option<RegistryStore>,
        seed: Vec<(ModelSpec, Option<u64>)>,
    ) -> Self {
        let memory_budget = scheduler::memory_budget();
        if let Some(budget) = memory_budget {
            tracing::info!(budget_gb = budget / (1024 * 1024 * 1024), "memory budget");
        }
        // 生产构造只装配真实引擎；mock / macos-say 是测试能力，
        // 由 new() 的 inject_test_providers 注入，不进生产 providers 表。
        let llama = Arc::new(LlamaCppProvider::from_env());
        let whisper = Arc::new(WhisperCppProvider::from_env());
        let kokoro = Arc::new(KokoroMlxProvider::from_env());
        let mlx_lm = Arc::new(MlxLmProvider::from_env());

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("llama.cpp".to_string(), llama.clone());
        providers.insert("whisper.cpp".to_string(), whisper.clone());
        providers.insert("kokoro-mlx".to_string(), kokoro.clone());
        providers.insert("mlx-lm".to_string(), mlx_lm.clone());

        let mut chat_providers: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        chat_providers.insert("llama.cpp".to_string(), llama);
        chat_providers.insert("mlx-lm".to_string(), mlx_lm);

        let mut stt_providers: HashMap<String, Arc<dyn STTProvider>> = HashMap::new();
        stt_providers.insert("whisper.cpp".to_string(), whisper);
        if qwen3_asr_enabled() {
            let qwen3_asr = Arc::new(Qwen3AsrProvider::from_env());
            let qwen3_asr_mlx = Arc::new(Qwen3AsrProvider::mlx_from_env());
            providers.insert("qwen3-asr".to_string(), qwen3_asr.clone());
            providers.insert("qwen3-asr-mlx".to_string(), qwen3_asr_mlx.clone());
            stt_providers.insert("qwen3-asr".to_string(), qwen3_asr);
            stt_providers.insert("qwen3-asr-mlx".to_string(), qwen3_asr_mlx);
        }

        let mut tts_providers: HashMap<String, Arc<dyn TTSProvider>> = HashMap::new();
        tts_providers.insert("kokoro-mlx".to_string(), kokoro);
        Self {
            registry: RwLock::new(seed_entries(seed)),
            store,
            memory_budget,
            providers,
            chat_providers,
            stt_providers,
            tts_providers,
            handles: RwLock::new(HashMap::new()),
            model_leases: StdMutex::new(HashMap::new()),
            lifecycle_lock: Mutex::new(()),
            started_at: Instant::now(),
            tasks: TaskRegistry::new(),
            request_counter: AtomicU64::new(0),
        }
    }

    /// 注册一个模型。注册不等于常驻；初始状态始终为 unloaded。
    /// 已存在时更新规格（保留状态与使用时间）。
    pub async fn register(&self, spec: ModelSpec) {
        let mut registry = self.registry.write().await;
        match registry.get_mut(&spec.id) {
            Some(entry) => entry.spec = spec.clone(),
            None => {
                registry.insert(
                    spec.id.clone(),
                    RegistryEntry {
                        spec: spec.clone(),
                        state: "unloaded".to_string(),
                        loaded_at: None,
                        last_used_at: None,
                    },
                );
            }
        }
        if let Some(store) = &self.store {
            store.upsert(&spec, unix_now(), None);
        }
    }

    pub async fn get_model(&self, id: &str) -> Option<ModelSpec> {
        self.registry
            .read()
            .await
            .get(id)
            .map(|entry| entry.spec.clone())
    }

    /// 重命名模型 ID。keep_alive / 上下文长度 / 默认音色等设置全部保留；
    /// 已加载的模型先卸载 worker（新 ID 下状态为 unloaded）。
    pub async fn rename_model(&self, id: &str, new_id: &str) -> Result<(), ProviderError> {
        let new_id = new_id.trim();
        if new_id.is_empty()
            || !new_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "invalid model id: only letters, digits, '.', '-', '_' are allowed",
            ));
        }
        {
            let registry = self.registry.read().await;
            if !registry.contains_key(id) {
                return Err(ProviderError::new(
                    AIError::ModelNotFound,
                    format!("model '{id}' not found"),
                ));
            }
            if registry.contains_key(new_id) {
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    format!("model id '{new_id}' already exists"),
                ));
            }
        }
        // 已加载则先卸载（在拿写锁之前做，避免同任务内锁升级死锁）。
        // 读守卫必须在 unload 前释放：if let 的 scrutinee 临时值会存活到整个
        // if let 结束，带着读锁调 unload_model 会在 set_state 的写锁上死锁。
        let loaded_state = {
            let registry = self.registry.read().await;
            registry.get(id).map(|entry| entry.state.clone())
        };
        if matches!(
            loaded_state.as_deref(),
            Some("loading" | "ready" | "busy" | "idle" | "unloading")
        ) {
            self.unload_model(id).await?;
        }
        let mut entry = {
            let mut registry = self.registry.write().await;
            registry.remove(id)
        };
        if let Some(ref mut entry) = entry {
            entry.spec.id = new_id.to_string();
            entry.state = "unloaded".to_string();
            self.registry
                .write()
                .await
                .insert(new_id.to_string(), entry.clone());
            if let Some(store) = &self.store {
                store.upsert(&entry.spec, unix_now(), None);
                store.remove(id);
            }
        }
        Ok(())
    }

    /// 从注册表中删除一个模型。已加载的模型会先卸载（终止 worker）。
    /// 返回 Err 表示模型不存在或卸载失败。
    pub async fn unregister_model(&self, id: &str) -> Result<(), ProviderError> {
        if self.registry.read().await.get(id).is_none() {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("model '{id}' not found"),
            ));
        }
        // 已加载则先卸载，避免孤儿 worker。读守卫必须在 unload 前释放，理由同
        // rename_model：if let 的 scrutinee 临时值会在 unload 期间占住读锁，
        // 与 unload_model -> set_state 的写锁互等死锁。
        let loaded_state = {
            let registry = self.registry.read().await;
            registry.get(id).map(|entry| entry.state.clone())
        };
        if matches!(
            loaded_state.as_deref(),
            Some("loading" | "ready" | "busy" | "idle" | "unloading")
        ) {
            self.unload_model(id).await?;
        }
        self.registry.write().await.remove(id);
        if let Some(store) = &self.store {
            store.remove(id);
        }
        Ok(())
    }

    pub async fn list_models(&self) -> Vec<RegistryEntry> {
        let registry = self.registry.read().await;
        let mut entries: Vec<RegistryEntry> = registry.values().cloned().collect();
        entries.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
        entries
    }

    #[cfg(test)]
    pub fn provider_descriptors(&self) -> Vec<ProviderDescriptor> {
        let mut descriptors: Vec<_> = self
            .providers
            .values()
            .map(|provider| provider.descriptor())
            .collect();
        descriptors.sort_by(|a, b| a.id.cmp(&b.id));
        descriptors
    }

    pub async fn provider_statuses(&self) -> Vec<(ProviderDescriptor, ProviderStatus)> {
        let mut statuses = Vec::with_capacity(self.providers.len());
        for provider in self.providers.values() {
            statuses.push((provider.descriptor(), provider.status().await));
        }
        statuses.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        statuses
    }

    /// 注册并加载一个模型。生命周期串行化，避免两个模型同时争抢同一 Provider。
    pub async fn register_and_load(&self, spec: ModelSpec) -> Result<ModelHandle, ProviderError> {
        if self.handles.read().await.contains_key(&spec.id) {
            self.unload_model(&spec.id).await?;
        }
        self.register(spec.clone()).await;
        self.load_model(&spec.id).await
    }

    pub async fn load_model(&self, id: &str) -> Result<ModelHandle, ProviderError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        let spec = self.get_model(id).await.ok_or_else(|| {
            ProviderError::new(AIError::ModelNotFound, format!("model '{id}' not found"))
        })?;

        // 内存预算检查（handoff §24）：预算不足先 LRU 逐出，仍不足则拒绝。
        // 无 estimate 的小模型（mock 等）按 0 计，不受预算约束。
        if self.memory_budget.is_some() {
            let requested = spec.memory_estimate.unwrap_or(0);
            if requested > 0 && !self.evict_for_memory(requested, Some(id)).await {
                return Err(ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!(
                        "insufficient AI memory budget for model '{id}' \
                         (need ~{} bytes after eviction)",
                        requested
                    ),
                ));
            }
        }

        let provider = self.providers.get(&spec.provider).cloned().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                format!("provider '{}' is not registered", spec.provider),
            )
        })?;

        let existing_handle = {
            let handles = self.handles.read().await;
            handles.get(id).cloned()
        };
        if let Some(handle) = existing_handle {
            let status = provider.status().await;
            if status_confirms_resident(&status, id) {
                return Ok(handle);
            }
            tracing::warn!(
                model = id,
                provider = %spec.provider,
                reason = ?status.reason,
                "discarding stale model handle and reloading provider"
            );
            self.handles.write().await.remove(id);
            self.set_state(id, "failed", false).await;
        }

        // 一个 Provider 首版只持有一个模型。切换模型时先释放同 Provider 的旧句柄。
        let prior: Vec<(String, ModelHandle)> = self
            .handles
            .read()
            .await
            .iter()
            .filter(|(_, handle)| handle.provider_id == spec.provider)
            .map(|(model_id, handle)| (model_id.clone(), handle.clone()))
            .collect();
        for (model_id, handle) in prior {
            self.ensure_model_idle(&model_id)?;
            provider.unload(&handle).await?;
            self.handles.write().await.remove(&model_id);
            self.set_state(&model_id, "unloaded", false).await;
        }

        self.set_state(id, "loading", false).await;
        match provider.load(&spec).await {
            Ok(handle) => {
                self.handles
                    .write()
                    .await
                    .insert(id.to_string(), handle.clone());
                self.set_state(id, "ready", true).await;
                Ok(handle)
            }
            Err(error) => {
                self.set_state(id, "failed", false).await;
                Err(error)
            }
        }
    }

    pub async fn unload_model(&self, id: &str) -> Result<(), ProviderError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_model_idle(id)?;
        let handle = self.handles.read().await.get(id).cloned();
        let Some(handle) = handle else {
            self.set_state(id, "unloaded", false).await;
            return Ok(());
        };
        let provider = self
            .providers
            .get(&handle.provider_id)
            .cloned()
            .ok_or_else(|| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!("provider '{}' is not registered", handle.provider_id),
                )
            })?;
        self.set_state(id, "unloading", false).await;
        match provider.unload(&handle).await {
            Ok(()) => {
                self.handles.write().await.remove(id);
                self.set_state(id, "unloaded", false).await;
                Ok(())
            }
            Err(error) => {
                self.set_state(id, "failed", false).await;
                Err(error)
            }
        }
    }

    /// 解析模型并确保常驻，返回其 Chat Provider。
    pub async fn chat_provider(
        &self,
        model_id: &str,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        self.load_model(model_id).await?;
        let spec = self.get_model(model_id).await.ok_or_else(|| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("model '{model_id}' not found"),
            )
        })?;
        self.chat_providers
            .get(&spec.provider)
            .cloned()
            .ok_or_else(|| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!("provider '{}' does not support chat", spec.provider),
                )
            })
    }

    pub async fn transcribe(
        self: &Arc<Self>,
        request: TranscriptionRequest,
        task: TaskHandle,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let model_id = request.model.clone();
        let _lease = self.model_lease(&model_id);
        let result: Result<TranscriptionResponse, ProviderError> = async {
            let spec = self.get_model(&model_id).await.ok_or_else(|| {
                ProviderError::new(
                    AIError::ModelNotFound,
                    format!("model '{model_id}' not found"),
                )
            })?;
            task.set_provider(Some(spec.provider.clone()));
            self.load_model(&model_id).await?;
            let provider = self
                .stt_providers
                .get(&spec.provider)
                .cloned()
                .ok_or_else(|| {
                    ProviderError::new(
                        AIError::ProviderUnavailable,
                        format!("provider '{}' does not support STT", spec.provider),
                    )
                })?;
            provider.transcribe(request).await
        }
        .await;
        match result {
            Ok(response) => {
                task.succeed(TaskResultDetail {
                    output_text: Some(response.text.clone()),
                    language: response.language.clone(),
                    ..TaskResultDetail::default()
                });
                self.touch_model(&model_id).await;
                Ok(response)
            }
            Err(error) => {
                task.fail(error.message.clone());
                Err(error)
            }
        }
    }

    pub async fn synthesize(
        self: &Arc<Self>,
        mut request: SpeechRequest,
        task: TaskHandle,
    ) -> Result<SpeechResponse, ProviderError> {
        let model_id = request.model.clone();
        let _lease = self.model_lease(&model_id);
        let result: Result<SpeechResponse, ProviderError> = async {
            let spec = self.get_model(&model_id).await.ok_or_else(|| {
                ProviderError::new(
                    AIError::ModelNotFound,
                    format!("model '{model_id}' not found"),
                )
            })?;
            task.set_provider(Some(spec.provider.clone()));
            self.load_model(&model_id).await?;
            if request.voice.is_none() {
                request.voice = spec.default_voice.clone();
            }
            task.update_request(|detail| detail.voice = request.voice.clone());
            let provider = self
                .tts_providers
                .get(&spec.provider)
                .cloned()
                .ok_or_else(|| {
                    ProviderError::new(
                        AIError::ProviderUnavailable,
                        format!("provider '{}' does not support TTS", spec.provider),
                    )
                })?;
            provider.synthesize(request).await
        }
        .await;
        match result {
            Ok(response) => {
                task.succeed(TaskResultDetail {
                    content_type: Some(response.content_type.clone()),
                    byte_count: Some(response.bytes),
                    ..TaskResultDetail::default()
                });
                self.touch_model(&model_id).await;
                Ok(response)
            }
            Err(error) => {
                task.fail(error.message.clone());
                Err(error)
            }
        }
    }

    pub async fn touch_model(&self, id: &str) {
        let now = unix_now();
        if let Some(entry) = self.registry.write().await.get_mut(id) {
            entry.last_used_at = Some(now);
        }
        if let Some(store) = &self.store {
            store.touch(id, now);
        }
    }

    /// 更新模型的 keep_alive 策略。只改注册表与持久层，进程保持常驻；
    /// reaper 下一次扫描即按新值执行。返回是否找到该模型。
    pub async fn set_keep_alive(&self, id: &str, keep_alive: Option<String>) -> bool {
        let found = {
            let mut registry = self.registry.write().await;
            match registry.get_mut(id) {
                Some(entry) => {
                    entry.spec.keep_alive = keep_alive.clone();
                    true
                }
                None => false,
            }
        };
        if found {
            if let Some(store) = &self.store {
                store.set_keep_alive(id, keep_alive.as_deref());
            }
        }
        found
    }

    /// 更新 TTS 模型的默认音色。只改注册表与持久层，进程保持常驻。
    /// 返回是否找到该模型。
    pub async fn set_default_voice(&self, id: &str, voice: Option<String>) -> bool {
        let found = {
            let mut registry = self.registry.write().await;
            match registry.get_mut(id) {
                Some(entry) => {
                    entry.spec.default_voice = voice.clone();
                    true
                }
                None => false,
            }
        };
        if found {
            if let Some(store) = &self.store {
                store.set_default_voice(id, voice.as_deref());
            }
        }
        found
    }

    async fn set_state(&self, id: &str, state: &str, loaded: bool) {
        let now = unix_now();
        if let Some(entry) = self.registry.write().await.get_mut(id) {
            entry.state = state.to_string();
            if loaded {
                entry.loaded_at = Some(now);
                entry.last_used_at = Some(now);
            } else if state == "unloaded" {
                entry.loaded_at = None;
            }
        }
        if let Some(store) = &self.store {
            store.set_state(id, state);
        }
    }

    pub fn next_request_id(&self) -> String {
        let n = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        format!("req_{t}_{n}")
    }

    pub fn start_task(
        &self,
        kind: impl Into<String>,
        model: impl Into<String>,
        request: TaskRequestDetail,
    ) -> TaskHandle {
        self.tasks
            .start(self.next_request_id(), kind, model, request)
    }

    pub fn tasks(&self) -> TaskRegistry {
        self.tasks.clone()
    }

    pub fn model_lease(self: &Arc<Self>, model_id: &str) -> ModelLease {
        if let Ok(mut leases) = self.model_leases.lock() {
            *leases.entry(model_id.to_string()).or_insert(0) += 1;
        }
        ModelLease {
            runtime: Arc::clone(self),
            model_id: model_id.to_string(),
        }
    }

    fn active_model_leases(&self, model_id: &str) -> u64 {
        self.model_leases
            .lock()
            .map(|leases| leases.get(model_id).copied().unwrap_or(0))
            .unwrap_or(1)
    }

    fn ensure_model_idle(&self, model_id: &str) -> Result<(), ProviderError> {
        let active = self.active_model_leases(model_id);
        if active == 0 {
            Ok(())
        } else {
            Err(ProviderError::new(
                AIError::ProviderUnavailable,
                format!("model '{model_id}' is busy with {active} active request(s)"),
            ))
        }
    }

    /// 当前常驻模型的内存占用合计（estimate；无 estimate 的按 0 计）。
    /// exclude_id 用于把自己排除——load_model 重入时目标模型可能已常驻。
    async fn resident_memory_bytes(&self, exclude_id: Option<&str>) -> u64 {
        self.registry
            .read()
            .await
            .values()
            .filter(|entry| entry.state != "unloaded" && entry.state != "failed")
            .filter(|entry| Some(entry.spec.id.as_str()) != exclude_id)
            .filter_map(|entry| entry.spec.memory_estimate)
            .sum()
    }

    /// LRU 逐出（handoff §24）：预算不足时按 last_used 升序逐出无 lease 的
    /// 常驻模型，直到腾出 requested 字节。keep_alive=always 的不逐。
    /// 必须在 lifecycle_lock 内调用。返回 true 表示已腾出足够空间。
    async fn evict_for_memory(&self, requested: u64, exclude_id: Option<&str>) -> bool {
        let Some(budget) = self.memory_budget else {
            return true; // 无法探测内存：跳过约束
        };
        let resident = self.resident_memory_bytes(exclude_id).await;
        if resident.saturating_add(requested) <= budget {
            return true;
        }
        let need = resident.saturating_add(requested) - budget;
        tracing::info!(
            need,
            budget,
            "memory budget exceeded; starting LRU eviction"
        );

        // 候选：常驻、非 busy、无 lease、keep_alive 非 always、不是目标模型本身。
        let mut candidates: Vec<(String, Option<u64>)> = self
            .registry
            .read()
            .await
            .values()
            .filter(|entry| {
                matches!(
                    entry.state.as_str(),
                    "ready" | "idle" | "busy" | "loading" | "unloading"
                ) && entry.state != "busy"
                    && entry.spec.keep_alive.as_deref() != Some("always")
            })
            .map(|entry| (entry.spec.id.clone(), entry.last_used_at))
            .collect();
        candidates.sort_by_key(|(_, last_used)| (*last_used, std::cmp::Ordering::Greater));

        let mut freed = 0u64;
        for (id, _) in candidates {
            if freed >= need {
                break;
            }
            if self.active_model_leases(&id) > 0 {
                continue;
            }
            let bytes = self
                .registry
                .read()
                .await
                .get(&id)
                .and_then(|entry| entry.spec.memory_estimate)
                .unwrap_or(0);
            match self.unload_model(&id).await {
                Ok(()) => {
                    tracing::info!(model = %id, freed_bytes = bytes, "LRU evicted model");
                    freed += bytes;
                }
                Err(error) => {
                    tracing::warn!(model = %id, %error, "LRU eviction failed");
                }
            }
        }
        freed >= need
    }

    /// keep-alive reaper 的单次扫描（handoff §25）：卸载空闲超过 keep_alive
    /// 且无活跃 lease 的常驻模型。"always" 与无法解析的值永不自动卸载。
    pub async fn reap_idle_models(&self) {
        let now = unix_now();
        let expired: Vec<String> = self
            .registry
            .read()
            .await
            .values()
            .filter(|entry| {
                matches!(entry.state.as_str(), "ready" | "idle")
                    && entry
                        .spec
                        .keep_alive
                        .as_deref()
                        .map(|value| scheduler::parse_keep_alive(Some(value)))
                        .unwrap_or(None)
                        .is_some_and(|ttl| {
                            ttl == 0
                                || entry
                                    .last_used_at
                                    .is_some_and(|last| now.saturating_sub(last) >= ttl)
                        })
            })
            .map(|entry| entry.spec.id.clone())
            .collect();

        for id in expired {
            if self.active_model_leases(&id) > 0 {
                continue;
            }
            match self.unload_model(&id).await {
                Ok(()) => tracing::info!(model = %id, "keep_alive expired; model unloaded"),
                Err(error) => {
                    tracing::warn!(model = %id, %error, "keep_alive unload failed")
                }
            }
        }
    }

    /// 启动 keep-alive 后台任务。由 main 调用一次。
    pub fn spawn_idle_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                ticker.tick().await;
                runtime.reap_idle_models().await;
            }
        })
    }

    pub async fn runtime_info(&self, version: &str) -> RuntimeInfo {
        let entries = self.list_models().await.into_iter().filter(|entry| {
            matches!(
                entry.state.as_str(),
                "loading" | "ready" | "busy" | "idle" | "unloading"
            )
        });
        let mut loaded_models = Vec::new();
        for entry in entries {
            let memory_usage_bytes = match self.providers.get(&entry.spec.provider) {
                Some(provider) => provider.memory_usage_bytes().await,
                None => None,
            };
            let effective_device = match self.providers.get(&entry.spec.provider) {
                Some(provider) => provider.effective_device().await,
                None => None,
            };
            loaded_models.push(LoadedModelInfo {
                id: entry.spec.id,
                provider: entry.spec.provider,
                state: entry.state,
                memory_estimate: entry.spec.memory_estimate,
                memory_usage_bytes,
                keep_alive: entry.spec.keep_alive,
                loaded_at: entry.loaded_at,
                last_used_at: entry.last_used_at,
                context_length: entry.spec.context_length,
                model_type: Some(entry.spec.model_type),
                default_voice: entry.spec.default_voice,
                effective_device,
            });
        }
        RuntimeInfo {
            version: version.to_string(),
            pid: std::process::id(),
            uptime_secs: self.started_at.elapsed().as_secs(),
            loaded_models,
            active_requests: self.tasks.running_count(),
            memory_budget: self.memory_budget,
            memory_total: scheduler::total_memory_bytes(),
            memory_used: scheduler::used_memory_bytes(),
        }
    }

    /// 优雅关闭：卸载所有仍在驻留的模型（终止对应 worker 进程），
    /// daemon 收到 SIGTERM/SIGINT 时调用，避免 worker 成为孤儿。
    pub async fn shutdown_all(&self) {
        let loaded_ids: Vec<String> = self.handles.read().await.keys().cloned().collect();
        for id in loaded_ids {
            match self.unload_model(&id).await {
                Ok(()) => tracing::info!(model = %id, "shutdown: unloaded model"),
                Err(error) => tracing::warn!(model = %id, %error, "shutdown: unload failed"),
            }
        }
        tracing::info!("shutdown complete");
    }
}

fn qwen3_asr_enabled() -> bool {
    let value = std::env::var("AIWORK_QWEN3_ASR_ENABLED").ok();
    qwen3_asr_enabled_value(value.as_deref())
}

fn qwen3_asr_enabled_value(value: Option<&str>) -> bool {
    value
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true)
}

fn status_confirms_resident(status: &ProviderStatus, model_id: &str) -> bool {
    status.ready
        && (status.resident_models.is_empty()
            || status
                .resident_models
                .iter()
                .any(|resident| resident == model_id))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 把持久层恢复的 (spec, last_used) 播种进内存注册表；状态一律 unloaded，
/// 因为 daemon 重启后没有任何 worker 进程存活。
fn seed_entries(seed: Vec<(ModelSpec, Option<u64>)>) -> HashMap<String, RegistryEntry> {
    let mut map = HashMap::new();
    for (spec, last_used_at) in seed {
        map.insert(
            spec.id.clone(),
            RegistryEntry {
                spec,
                state: "unloaded".to_string(),
                loaded_at: None,
                last_used_at,
            },
        );
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_spec() -> ModelSpec {
        ModelSpec {
            id: "mock-test".to_string(),
            name: "Mock".to_string(),
            model_type: "llm".to_string(),
            provider: "mock".to_string(),
            source: None,
            path: None,
            format: Some("mock".to_string()),
            size_bytes: Some(0),
            memory_estimate: Some(0),
            keep_alive: Some("always".to_string()),
            context_length: Some(4096),
            default_voice: None,
        }
    }

    #[test]
    fn provider_status_must_confirm_ready_resident_model() {
        let ready_mock = ProviderStatus {
            available: true,
            ready: true,
            effective_device: Some("cpu".to_string()),
            resident_models: vec![],
            reason: None,
            install_hint: None,
        };
        assert!(status_confirms_resident(&ready_mock, "mock"));

        let mut worker = ready_mock.clone();
        worker.resident_models = vec!["model-a".to_string()];
        assert!(status_confirms_resident(&worker, "model-a"));
        assert!(!status_confirms_resident(&worker, "model-b"));

        worker.ready = false;
        assert!(!status_confirms_resident(&worker, "model-a"));
    }

    #[tokio::test]
    async fn audio_paths_finalize_tasks_when_model_lookup_fails() {
        let runtime = Arc::new(Runtime::new());
        let stt_task = runtime.start_task("stt", "missing-stt", TaskRequestDetail::default());
        let stt_error = runtime
            .transcribe(
                TranscriptionRequest {
                    model: "missing-stt".to_string(),
                    file: Some("meeting.wav".to_string()),
                    language: Some("zh".to_string()),
                    response_format: Some("json".to_string()),
                },
                stt_task,
            )
            .await
            .unwrap_err();
        assert!(stt_error.message.contains("not found"));

        let tts_task = runtime.start_task("tts", "missing-tts", TaskRequestDetail::default());
        let tts_error = runtime
            .synthesize(
                SpeechRequest {
                    model: "missing-tts".to_string(),
                    input: "hello".to_string(),
                    voice: None,
                    format: Some("wav".to_string()),
                    speed: None,
                },
                tts_task,
            )
            .await
            .unwrap_err();
        assert!(tts_error.message.contains("not found"));

        assert_eq!(runtime.tasks().running_count(), 0);
        let completed = runtime.tasks().list(100).completed;
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].kind, "tts");
        assert_eq!(completed[0].status, "failed");
        assert_eq!(completed[1].kind, "stt");
        assert_eq!(completed[1].status, "failed");
    }

    #[tokio::test]
    async fn active_model_lease_blocks_unload() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_spec()).await;
        runtime.load_model("mock-test").await.unwrap();

        let lease = runtime.model_lease("mock-test");
        let error = runtime.unload_model("mock-test").await.unwrap_err();
        assert_eq!(error.kind, AIError::ProviderUnavailable);
        assert_eq!(runtime.runtime_info("test").await.loaded_models.len(), 1);

        drop(lease);
        runtime.unload_model("mock-test").await.unwrap();
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
    }

    #[tokio::test]
    async fn registered_model_is_not_resident_until_loaded() {
        let runtime = Runtime::new();
        runtime.register(mock_spec()).await;
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
        runtime.load_model("mock-test").await.unwrap();
        let info = runtime.runtime_info("test").await;
        assert_eq!(info.loaded_models.len(), 1);
        assert_eq!(info.loaded_models[0].state, "ready");
        runtime.unload_model("mock-test").await.unwrap();
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
    }

    #[test]
    fn exposes_all_provider_descriptors() {
        let runtime = Runtime::new();
        let ids: Vec<_> = runtime
            .provider_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "kokoro-mlx",
                "llama.cpp",
                "macos-say",
                "mlx-lm",
                "mock",
                "qwen3-asr",
                "qwen3-asr-mlx",
                "whisper.cpp"
            ]
        );
    }
}
