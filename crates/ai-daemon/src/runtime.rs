//! Runtime — daemon 唯一的模型与 Provider Authority。
//!
//! 内存 HashMap 是唯一读路径；SQLite 持久化注册，scheduler 统一处理内存预算、
//! lease、LRU 与 keep-alive。生产后端全部通过受监督 Runner 进程接入。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
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

use crate::providers::{MacOSSayProvider, MockProvider};
use crate::registry::{RegistryStore, StoredProfile};
use crate::scheduler;
use crate::tasks::{TaskHandle, TaskRegistry};
use ai_daemon::runners::RunnerInstanceManager;

/// 内存版模型注册表条目：模型规格 + 当前状态 + 使用时间。
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub spec: ModelSpec,
    /// catalog Runner 模型注册当时的 immutable Profile snapshot；ad-hoc Runner
    /// 模型为 None。
    pub profile: Option<StoredProfile>,
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
    /// Runner-backed 模型的进程/协议通道。None = 未配置 Runner。
    runner_instances: Option<Arc<RunnerInstanceManager>>,
    /// ad-hoc 绑定通道：attach 过的 RunnerProvider 按 runner id 索引。
    /// 注册路径据此为无 catalog Profile 的模型建立内存绑定。
    runner_providers: StdRwLock<HashMap<String, Arc<ai_daemon::runners::RunnerProvider>>>,
    /// discovery 得到的当前 Profile catalog。它只用于新注册；已注册模型继续使用
    /// RegistryEntry.profile 中冻结的 snapshot。
    runner_profiles: StdRwLock<HashMap<String, ai_daemon::runners::ModelProfile>>,
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
        let mut runtime = Self::with_options_and_seed(None, Vec::new(), HashMap::new());
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
        let restored_profiles = store
            .as_ref()
            .map(RegistryStore::load_model_profile_bindings)
            .unwrap_or_default();
        let runtime = Self::with_options_and_seed(store, restored.clone(), restored_profiles);
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
        registered_profiles: HashMap<String, StoredProfile>,
    ) -> Self {
        let memory_budget = scheduler::memory_budget();
        if let Some(budget) = memory_budget {
            tracing::info!(budget_gb = budget / (1024 * 1024 * 1024), "memory budget");
        }
        // 生产后端全部由 bootstrap_runners 动态装配；mock / macos-say 只由
        // new() 的测试构造注入，不进入生产 Provider 表。
        let providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        let chat_providers: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        let stt_providers: HashMap<String, Arc<dyn STTProvider>> = HashMap::new();
        let tts_providers: HashMap<String, Arc<dyn TTSProvider>> = HashMap::new();
        Self {
            registry: RwLock::new(seed_entries(seed, registered_profiles)),
            store,
            memory_budget,
            providers,
            chat_providers,
            stt_providers,
            tts_providers,
            handles: RwLock::new(HashMap::new()),
            model_leases: StdMutex::new(HashMap::new()),
            lifecycle_lock: Mutex::new(()),
            runner_instances: None,
            runner_providers: StdRwLock::new(HashMap::new()),
            runner_profiles: StdRwLock::new(HashMap::new()),
            started_at: Instant::now(),
            tasks: TaskRegistry::new(),
            request_counter: AtomicU64::new(0),
        }
    }

    pub fn runner_instances(&self) -> Option<Arc<RunnerInstanceManager>> {
        self.runner_instances.clone()
    }

    /// 查询 provider（含动态装配的 Runner）是否声明给定能力。注册/管理面校验用。
    pub fn provider_has_capability(
        &self,
        provider_id: &str,
        capability: ai_core::provider::Capability,
    ) -> bool {
        self.providers
            .get(provider_id)
            .is_some_and(|provider| provider.descriptor().capabilities.contains(&capability))
    }

    /// Runner-backed Provider 装配。main 构造 Runtime 后、Arc 包装前
    /// 调用：按 descriptor 注册进 providers 与 capability 表，并挂上 instance
    /// manager 供 `shutdown_all` 收口。一个 Runner 一个 RunnerProvider。
    pub fn attach_runner(
        &mut self,
        provider: Arc<ai_daemon::runners::RunnerProvider>,
        instances: Arc<ai_daemon::runners::RunnerInstanceManager>,
    ) {
        let descriptor = provider.descriptor();
        let id = descriptor.id.clone();
        self.runner_providers
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), provider.clone());
        self.providers.insert(id.clone(), provider.clone());
        if descriptor
            .capabilities
            .contains(&ai_core::provider::Capability::Chat)
        {
            self.chat_providers.insert(id.clone(), provider.clone());
        }
        if descriptor
            .capabilities
            .contains(&ai_core::provider::Capability::TextToSpeech)
        {
            self.tts_providers.insert(id.clone(), provider.clone());
        }
        if descriptor
            .capabilities
            .contains(&ai_core::provider::Capability::SpeechToText)
        {
            self.stt_providers.insert(id, provider);
        }
        self.runner_instances = Some(instances);
    }

    /// 持久化 Model Profile snapshot 到 models.db `model_profiles` 表。
    /// 返回内容 digest；内存模式（无 store）返回 Ok(None)。
    pub fn register_runner_profile(
        &self,
        profile: &ai_daemon::runners::ModelProfile,
    ) -> Result<Option<String>, String> {
        let digest = profile
            .digest()
            .map_err(|error| format!("profile digest failed: {error}"))?;
        let snapshot = profile
            .canonical_json()
            .map_err(|error| format!("profile snapshot failed: {error}"))?;
        let snapshot =
            String::from_utf8(snapshot).map_err(|_| "profile snapshot is not utf-8".to_string())?;
        let installed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        self.runner_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(profile.id.clone(), profile.clone());
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        store.upsert_profile(
            &profile.id,
            &profile.runner,
            &profile.compatibility.runner,
            &digest,
            &snapshot,
            installed_at,
        );
        Ok(Some(digest))
    }

    /// 返回某个已注册模型冻结的 Profile；用于 daemon 重启后重新建立 Runner binding。
    pub async fn registered_runner_profile(
        &self,
        model_id: &str,
    ) -> Option<ai_daemon::runners::ModelProfile> {
        let snapshot = self
            .registry
            .read()
            .await
            .get(model_id)
            .and_then(|entry| entry.profile.clone())?;
        ai_daemon::runners::ModelProfile::from_snapshot(&snapshot.snapshot).ok()
    }

    fn current_runner_profile(&self, model_id: &str, provider_id: &str) -> Option<StoredProfile> {
        let profile = self
            .runner_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(model_id)
            .filter(|profile| profile.runner == provider_id)
            .cloned()?;
        let digest = profile.digest().ok()?;
        let snapshot = String::from_utf8(profile.canonical_json().ok()?).ok()?;
        Some(StoredProfile {
            profile_id: profile.id,
            runner: profile.runner,
            compatibility: profile.compatibility.runner,
            digest,
            snapshot,
            installed_at: unix_now(),
        })
    }

    /// 一次性迁移旧库：已有 Runner-backed ModelSpec 若尚未绑定 Profile，就冻结当前
    /// catalog snapshot。已有绑定永不被 catalog 更新覆盖。
    pub async fn freeze_existing_runner_profile(
        &self,
        profile: &ai_daemon::runners::ModelProfile,
    ) -> Result<bool, String> {
        let frozen = self
            .current_runner_profile(&profile.id, &profile.runner)
            .ok_or_else(|| format!("profile '{}' is not in the current catalog", profile.id))?;
        let mut registry = self.registry.write().await;
        let Some(entry) = registry.get_mut(&profile.id) else {
            return Ok(false);
        };
        if entry.spec.provider != profile.runner || entry.profile.is_some() {
            return Ok(false);
        }
        entry.profile = Some(frozen.clone());
        drop(registry);
        if let Some(store) = &self.store {
            store.bind_model_profile(&profile.id, &frozen);
        }
        Ok(true)
    }

    /// 注册一个模型。注册不等于常驻；初始状态始终为 unloaded。
    /// 已存在时更新规格（保留状态与使用时间）。
    pub async fn register(&self, spec: ModelSpec) {
        self.register_with_profile(spec, None).await;
    }

    async fn register_with_profile(&self, spec: ModelSpec, profile: Option<StoredProfile>) {
        let mut registry = self.registry.write().await;
        match registry.get_mut(&spec.id) {
            Some(entry) => {
                entry.spec = spec.clone();
                if profile.is_some() {
                    entry.profile = profile.clone();
                }
            }
            None => {
                registry.insert(
                    spec.id.clone(),
                    RegistryEntry {
                        spec: spec.clone(),
                        profile: profile.clone(),
                        state: "unloaded".to_string(),
                        loaded_at: None,
                        last_used_at: None,
                    },
                );
            }
        }
        if let Some(store) = &self.store {
            store.upsert(&spec, unix_now(), None);
            if let Some(profile) = profile {
                store.bind_model_profile(&spec.id, &profile);
            }
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
            let Some(existing) = registry.get(id) else {
                return Err(ProviderError::new(
                    AIError::ModelNotFound,
                    format!("model '{id}' not found"),
                ));
            };
            if existing.profile.is_some() {
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    "Runner-backed Model Profile IDs are stable and cannot be renamed",
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
        let adhoc_runner_provider = {
            let registry = self.registry.read().await;
            registry.get(id).and_then(|entry| {
                (entry.profile.is_none() && entry.spec.provider.starts_with("org.macai."))
                    .then(|| entry.spec.provider.clone())
            })
        };
        if let Some(provider_id) = adhoc_runner_provider {
            let provider = self
                .runner_providers
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&provider_id)
                .cloned()
                .ok_or_else(|| {
                    ProviderError::new(
                        AIError::ProviderUnavailable,
                        format!("Runner provider '{provider_id}' is not attached"),
                    )
                })?;
            let renamed = provider
                .rename_bound_model(id, new_id)
                .await
                .map_err(|message| ProviderError::new(AIError::InvalidRequest, message))?;
            if !renamed {
                return Err(ProviderError::new(
                    AIError::ModelNotFound,
                    format!("Runner model binding '{id}' not found"),
                ));
            }
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
                if let Some(profile) = &entry.profile {
                    store.bind_model_profile(new_id, profile);
                }
                store.remove(id);
            }
        }
        Ok(())
    }

    /// 从注册表中删除一个模型。已加载的模型会先卸载（终止 worker）。
    /// 返回 Err 表示模型不存在或卸载失败。
    pub async fn unregister_model(&self, id: &str) -> Result<(), ProviderError> {
        let adhoc_runner_provider = self.registry.read().await.get(id).map(|entry| {
            (entry.profile.is_none() && entry.spec.provider.starts_with("org.macai."))
                .then(|| entry.spec.provider.clone())
        });
        let Some(adhoc_runner_provider) = adhoc_runner_provider else {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("model '{id}' not found"),
            ));
        };
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
        if let Some(provider_id) = adhoc_runner_provider {
            let provider = {
                self.runner_providers
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&provider_id)
                    .cloned()
            };
            if let Some(provider) = provider {
                provider.unbind_model(id).await;
            }
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
    /// 新注册若加载失败会回滚注册（runner 绑定校验等失败不留脏条目）；
    /// 已存在模型的修复路径（加载失败重试）保留注册以便 GUI 重试。
    pub async fn register_and_load(&self, spec: ModelSpec) -> Result<ModelHandle, ProviderError> {
        if self.handles.read().await.contains_key(&spec.id) {
            self.unload_model(&spec.id).await?;
        }
        let existing_profile = self
            .registry
            .read()
            .await
            .get(&spec.id)
            .and_then(|entry| entry.profile.clone());
        let existed_before = self.get_model(&spec.id).await.is_some();
        let profile =
            existing_profile.or_else(|| self.current_runner_profile(&spec.id, &spec.provider));
        self.register_with_profile(spec.clone(), profile).await;
        match self.load_model(&spec.id).await {
            Ok(handle) => Ok(handle),
            Err(error) => {
                if !existed_before {
                    let _ = self.unregister_model(&spec.id).await;
                }
                Err(error)
            }
        }
    }

    /// ad-hoc Runner 绑定（无 catalog Profile 的引擎，如 llama.cpp）：在注册
    /// 路径上为模型建立内存绑定。catalog 已绑定（registry snapshot 或当前
    /// catalog 命中）时跳过，返回 false。
    pub async fn bind_adhoc_runner_model(
        &self,
        provider_id: &str,
        model_id: &str,
        model_path: &std::path::Path,
        adapter: &str,
        format: &str,
    ) -> Result<bool, String> {
        // 只把「有 catalog Profile snapshot」视为已绑定。spec.provider 匹配
        // 不能算：daemon 重启后注册记录从 SQLite 恢复，但 RunnerProvider 的
        // 内存绑定表是空的，必须重新建立 ad-hoc 绑定。
        let already_profiled = {
            let registry = self.registry.read().await;
            registry
                .get(model_id)
                .is_some_and(|entry| entry.profile.is_some())
        } || self
            .runner_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(model_id);
        if already_profiled {
            return Ok(false);
        }
        let provider = self
            .runner_providers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(provider_id)
            .cloned()
            .ok_or_else(|| format!("runner provider '{provider_id}' is not attached"))?;
        provider
            .bind_adhoc_model(model_id, model_path, adapter, format)
            .await
    }

    pub async fn load_model(&self, id: &str) -> Result<ModelHandle, ProviderError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        let spec = self.get_model(id).await.ok_or_else(|| {
            ProviderError::new(AIError::ModelNotFound, format!("model '{id}' not found"))
        })?;

        // 内存预算不足时先按 scheduler 规则 LRU 逐出，仍不足则拒绝。
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
            tracing::info!(
                model = %model_id,
                provider = %spec.provider,
                "audio request routed to provider (from model spec)"
            );
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
            tracing::info!(
                model = %model_id,
                provider = %spec.provider,
                "audio request routed to provider (from model spec)"
            );
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

    /// LRU 逐出：预算不足时按 last_used 升序逐出无 lease 的
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

    /// keep-alive reaper 的单次扫描：卸载空闲超过 keep_alive
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
                requested_provider: entry.spec.requested_provider,
                provider_selection_reason: entry.spec.provider_selection_reason,
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
    /// Runner instance 的 shutdown 也在此收口。
    pub async fn shutdown_all(&self) {
        let loaded_ids: Vec<String> = self.handles.read().await.keys().cloned().collect();
        for id in loaded_ids {
            match self.unload_model(&id).await {
                Ok(()) => tracing::info!(model = %id, "shutdown: unloaded model"),
                Err(error) => tracing::warn!(model = %id, %error, "shutdown: unload failed"),
            }
        }
        if let Some(instances) = &self.runner_instances {
            ai_daemon::runners::shutdown_all_instances(instances).await;
        }
        tracing::info!("shutdown complete");
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
fn seed_entries(
    seed: Vec<(ModelSpec, Option<u64>)>,
    mut registered_profiles: HashMap<String, StoredProfile>,
) -> HashMap<String, RegistryEntry> {
    let mut map = HashMap::new();
    for (spec, last_used_at) in seed {
        map.insert(
            spec.id.clone(),
            RegistryEntry {
                profile: registered_profiles.remove(&spec.id),
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
            requested_provider: Some("mock".to_string()),
            provider_selection_reason: Some("test provider selection".to_string()),
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
        assert_eq!(
            info.loaded_models[0].requested_provider.as_deref(),
            Some("mock")
        );
        assert_eq!(
            info.loaded_models[0].provider_selection_reason.as_deref(),
            Some("test provider selection")
        );
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
        assert_eq!(ids, vec!["macos-say", "mock"]);
    }

    #[tokio::test]
    async fn attach_runner_registers_provider_and_shutdown_handle() {
        use std::collections::HashSet;

        use ai_daemon::runners::{
            EnvironmentManager, EnvironmentManagerConfig, RunnerInstanceManager, RunnerProvider,
            RunnerRegistry,
        };

        let root = std::env::temp_dir().join(format!(
            "macai-runtime-attach-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        let environments = EnvironmentManager::new(EnvironmentManagerConfig {
            runtime_root: root.join("Runtimes/python"),
        });
        let instances = Arc::new(RunnerInstanceManager::new(
            registry,
            environments,
            root.join("temp"),
        ));
        let provider = Arc::new(RunnerProvider::new(
            "org.example.runner".to_string(),
            &["tts.v1".to_string()],
            instances.clone(),
            root.join("temp"),
        ));

        let mut runtime = Runtime::new();
        runtime.attach_runner(provider, instances.clone());
        assert!(
            runtime.providers.contains_key("org.example.runner"),
            "runner provider must be registered in the providers table"
        );
        assert!(
            runtime.tts_providers.contains_key("org.example.runner"),
            "runner provider must be registered as a TTS provider"
        );
        assert!(runtime.runner_instances.is_some());
        assert!(
            runtime.provider_has_capability(
                "org.example.runner",
                ai_core::provider::Capability::TextToSpeech
            ),
            "capability lookup must see dynamically attached providers"
        );
        assert!(
            !runtime.provider_has_capability(
                "org.example.runner",
                ai_core::provider::Capability::SpeechToText
            ),
            "capability lookup must not over-report"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn adhoc_runner_binding_tracks_refresh_rename_and_unregister() {
        use std::collections::HashSet;

        use ai_daemon::runners::{
            EnvironmentManager, EnvironmentManagerConfig, RunnerInstanceManager, RunnerProvider,
            RunnerRegistry,
        };

        let root = std::env::temp_dir().join(format!(
            "macai-runtime-adhoc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first_path = root.join("first.gguf");
        let second_path = root.join("second.gguf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&first_path, b"first").unwrap();
        std::fs::write(&second_path, b"second").unwrap();

        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        let environments = EnvironmentManager::new(EnvironmentManagerConfig {
            runtime_root: root.join("Runtimes/python"),
        });
        let instances = Arc::new(RunnerInstanceManager::new(
            registry,
            environments,
            root.join("temp"),
        ));
        let provider = Arc::new(RunnerProvider::new(
            "org.macai.test-runner".to_string(),
            &["chat.v1".to_string()],
            instances.clone(),
            root.join("temp"),
        ));
        let mut runtime = Runtime::new();
        runtime.attach_runner(provider.clone(), instances);

        assert!(runtime
            .bind_adhoc_runner_model(
                "org.macai.test-runner",
                "local-model",
                &first_path,
                "test-adapter",
                "gguf",
            )
            .await
            .unwrap());
        assert!(!runtime
            .bind_adhoc_runner_model(
                "org.macai.test-runner",
                "local-model",
                &second_path,
                "test-adapter",
                "gguf",
            )
            .await
            .unwrap());

        let mut spec = mock_spec();
        spec.id = "local-model".to_string();
        spec.provider = "org.macai.test-runner".to_string();
        spec.requested_provider = Some("org.macai.test-runner".to_string());
        spec.path = Some(second_path.display().to_string());
        runtime.register(spec.clone()).await;

        let refresh_error = provider.load(&spec).await.unwrap_err();
        assert!(!refresh_error.message.contains("not bound"));
        assert!(!refresh_error.message.contains("does not match Profile"));

        runtime
            .rename_model("local-model", "renamed-model")
            .await
            .unwrap();
        spec.id = "renamed-model".to_string();
        let rename_error = provider.load(&spec).await.unwrap_err();
        assert!(!rename_error.message.contains("not bound"));

        runtime.unregister_model("renamed-model").await.unwrap();
        let removed_error = provider.load(&spec).await.unwrap_err();
        assert_eq!(removed_error.kind, AIError::ModelNotFound);
        assert!(removed_error.message.contains("not bound"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn registered_runner_model_restores_its_immutable_profile_snapshot() {
        let path = std::env::temp_dir().join(format!(
            "macai-runtime-profile-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let profile_toml = r#"
schema = "macai.model.v1"
id = "kokoro-82m-zh"
name = "Kokoro 82M zh"
capabilities = ["tts.v1"]
runner = "org.macai.kokoro"
adapter = "kokoro-mlx"
format = "directory"
[source]
type = "huggingface"
repo = "1038lab/Kokoro-82M-zh-MLX"
revision = "4bd6c9644da381fee105f37fcd8cb63d038ba7e8"
[artifacts]
directory = "kokoro-82m-zh"
files = ["config.json", "model.safetensors"]
[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"
[resources]
memory_estimate_bytes = 400000000
[compatibility]
runner = ">=0.1,<0.2"
"#;
        let profile = ai_daemon::runners::ModelProfile::parse(profile_toml).unwrap();
        let runtime = Runtime::with_store(&path);
        let digest = runtime
            .register_runner_profile(&profile)
            .expect("register must succeed")
            .expect("store-backed runtime must persist");
        assert_eq!(digest.len(), 64);

        let spec = ModelSpec {
            id: profile.id.clone(),
            name: profile.name.clone(),
            model_type: "tts".to_string(),
            provider: profile.runner.clone(),
            requested_provider: Some(profile.runner.clone()),
            provider_selection_reason: Some("explicit provider selection".to_string()),
            source: None,
            path: Some("/Models/tts/kokoro-82m-zh".to_string()),
            format: Some(profile.format.clone()),
            size_bytes: None,
            memory_estimate: profile.resources.memory_estimate_bytes,
            keep_alive: profile.defaults.keep_alive.clone(),
            context_length: None,
            default_voice: profile.defaults.voice.clone(),
        };
        let frozen = runtime
            .current_runner_profile(&profile.id, &profile.runner)
            .expect("catalog profile");
        runtime.register(spec).await;
        assert!(runtime
            .freeze_existing_runner_profile(&profile)
            .await
            .expect("existing registration snapshot migration"));
        assert!(!runtime
            .freeze_existing_runner_profile(&profile)
            .await
            .expect("existing binding is immutable"));
        drop(runtime);

        let restored = Runtime::with_store(&path);
        let restored_profile = restored
            .registered_runner_profile(&profile.id)
            .await
            .expect("registered profile must survive restart");
        assert_eq!(restored_profile.digest().unwrap(), digest);
        let rename_error = restored
            .rename_model(&profile.id, "renamed-runner-model")
            .await
            .unwrap_err();
        assert_eq!(rename_error.kind, AIError::InvalidRequest);
        let store = crate::registry::RegistryStore::open(&path).unwrap();
        let profiles = store.load_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].profile_id, "kokoro-82m-zh");
        assert_eq!(profiles[0].runner, "org.macai.kokoro");
        assert_eq!(profiles[0].digest, digest);
        assert_eq!(
            store.load_model_profile_bindings()[&profile.id].digest,
            frozen.digest
        );
        drop(restored);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
