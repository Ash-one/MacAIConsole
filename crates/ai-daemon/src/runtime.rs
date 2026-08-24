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
use ai_core::response::{LoadedModelInfo, RuntimeInfo, SpeechResponse, TranscriptionResponse};
use ai_core::AIError;

use crate::providers::{KokoroMlxProvider, LlamaCppProvider, MockProvider, WhisperCppProvider};
use crate::registry::RegistryStore;
use crate::scheduler;

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
    active_requests: AtomicU64,
    request_counter: AtomicU64,
}

/// 活跃请求计数守卫。普通请求返回、流式响应结束或客户端断开时自动归零。
pub struct RequestGuard {
    runtime: Arc<Runtime>,
}

pub struct ModelLease {
    runtime: Arc<Runtime>,
    model_id: String,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.runtime.active_requests.fetch_sub(1, Ordering::SeqCst);
    }
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
    /// 纯内存构造，仅供单元测试：额外注入 mock provider，
    /// 生产路径（with_store）只包含真实引擎。
    pub fn new() -> Self {
        let mut runtime = Self::with_options_and_seed(None, Vec::new());
        let mock = Arc::new(MockProvider);
        runtime.providers.insert("mock".to_string(), mock.clone());
        runtime.chat_providers.insert("mock".to_string(), mock);
        runtime
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
        let llama = Arc::new(LlamaCppProvider::from_env());
        let whisper = Arc::new(WhisperCppProvider::from_env());
        let kokoro = Arc::new(KokoroMlxProvider::from_env());

        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert("llama.cpp".to_string(), llama.clone());
        providers.insert("whisper.cpp".to_string(), whisper.clone());
        providers.insert("kokoro-mlx".to_string(), kokoro.clone());

        let mut chat_providers: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        chat_providers.insert("llama.cpp".to_string(), llama);

        let mut stt_providers: HashMap<String, Arc<dyn STTProvider>> = HashMap::new();
        stt_providers.insert("whisper.cpp".to_string(), whisper);

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
            active_requests: AtomicU64::new(0),
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
            if requested > 0 && !self.evict_for_memory(requested).await {
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
    ) -> Result<TranscriptionResponse, ProviderError> {
        let model_id = request.model.clone();
        let _lease = self.model_lease(&model_id);
        let _request = self.request_guard();
        self.load_model(&model_id).await?;
        let spec = self.get_model(&model_id).await.ok_or_else(|| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("model '{model_id}' not found"),
            )
        })?;
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
        let response = provider.transcribe(request).await?;
        self.touch_model(&model_id).await;
        Ok(response)
    }

    pub async fn synthesize(
        self: &Arc<Self>,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        let model_id = request.model.clone();
        let _lease = self.model_lease(&model_id);
        let _request = self.request_guard();
        self.load_model(&model_id).await?;
        let spec = self.get_model(&model_id).await.ok_or_else(|| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("model '{model_id}' not found"),
            )
        })?;
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
        let response = provider.synthesize(request).await?;
        self.touch_model(&model_id).await;
        Ok(response)
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

    pub fn request_guard(self: &Arc<Self>) -> RequestGuard {
        self.active_requests.fetch_add(1, Ordering::SeqCst);
        RequestGuard {
            runtime: Arc::clone(self),
        }
    }

    /// 当前常驻模型的内存占用合计（estimate；无 estimate 的按 0 计）。
    async fn resident_memory_bytes(&self) -> u64 {
        self.registry
            .read()
            .await
            .values()
            .filter(|entry| entry.state != "unloaded" && entry.state != "failed")
            .filter_map(|entry| entry.spec.memory_estimate)
            .sum()
    }

    /// LRU 逐出（handoff §24）：预算不足时按 last_used 升序逐出无 lease 的
    /// 常驻模型，直到腾出 requested 字节。keep_alive=always 的不逐。
    /// 必须在 lifecycle_lock 内调用。返回 true 表示已腾出足够空间。
    async fn evict_for_memory(&self, requested: u64) -> bool {
        let Some(budget) = self.memory_budget else {
            return true; // 无法探测内存：跳过约束
        };
        let resident = self.resident_memory_bytes().await;
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
        let loaded_models = self
            .list_models()
            .await
            .into_iter()
            .filter(|entry| {
                matches!(
                    entry.state.as_str(),
                    "loading" | "ready" | "busy" | "idle" | "unloading"
                )
            })
            .map(|entry| LoadedModelInfo {
                id: entry.spec.id,
                provider: entry.spec.provider,
                state: entry.state,
                memory_estimate: entry.spec.memory_estimate,
                keep_alive: entry.spec.keep_alive,
                loaded_at: entry.loaded_at,
                last_used_at: entry.last_used_at,
            })
            .collect();
        RuntimeInfo {
            version: version.to_string(),
            pid: std::process::id(),
            uptime_secs: self.started_at.elapsed().as_secs(),
            loaded_models,
            active_requests: self.active_requests.load(Ordering::SeqCst),
            memory_budget: self.memory_budget,
        }
    }
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
            provider: "llama.cpp".to_string(),
            source: None,
            path: None,
            format: Some("gguf".to_string()),
            size_bytes: Some(0),
            memory_estimate: Some(0),
            keep_alive: Some("always".to_string()),
            context_length: Some(4096),
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
    async fn request_guard_tracks_and_releases_active_request() {
        let runtime = Arc::new(Runtime::new());
        assert_eq!(runtime.runtime_info("test").await.active_requests, 0);
        let guard = runtime.request_guard();
        assert_eq!(runtime.runtime_info("test").await.active_requests, 1);
        drop(guard);
        assert_eq!(runtime.runtime_info("test").await.active_requests, 0);
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
            vec!["kokoro-mlx", "llama.cpp", "whisper.cpp"]
        );
    }
}
