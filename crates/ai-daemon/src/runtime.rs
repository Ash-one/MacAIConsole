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
    ChatProvider, ModelHandle, ProviderDescriptor, ProviderError, ProviderStatus,
};
use ai_core::response::{LoadedModelInfo, RuntimeInfo};
use ai_core::AIError;

use crate::providers::{LlamaCppProvider, MockProvider};

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
    providers: HashMap<String, Arc<dyn ChatProvider>>,
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
    pub fn new() -> Self {
        let mut providers: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        providers.insert("mock".to_string(), Arc::new(MockProvider));
        providers.insert(
            "llama.cpp".to_string(),
            Arc::new(LlamaCppProvider::from_env()),
        );
        Self {
            registry: RwLock::new(HashMap::new()),
            providers,
            handles: RwLock::new(HashMap::new()),
            model_leases: StdMutex::new(HashMap::new()),
            lifecycle_lock: Mutex::new(()),
            started_at: Instant::now(),
            active_requests: AtomicU64::new(0),
            request_counter: AtomicU64::new(0),
        }
    }

    /// 注册一个模型。注册不等于常驻；初始状态始终为 unloaded。
    pub async fn register(&self, spec: ModelSpec) {
        let mut registry = self.registry.write().await;
        registry.insert(
            spec.id.clone(),
            RegistryEntry {
                spec,
                state: "unloaded".to_string(),
                loaded_at: None,
                last_used_at: None,
            },
        );
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
        self.providers.get(&spec.provider).cloned().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                format!("provider '{}' is not registered", spec.provider),
            )
        })
    }

    pub async fn touch_model(&self, id: &str) {
        let now = unix_now();
        if let Some(entry) = self.registry.write().await.get_mut(id) {
            entry.last_used_at = Some(now);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_spec() -> ModelSpec {
        ModelSpec {
            id: "mock".to_string(),
            name: "Mock".to_string(),
            model_type: "llm".to_string(),
            provider: "mock".to_string(),
            source: None,
            path: None,
            format: Some("mock".to_string()),
            size_bytes: None,
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
        runtime.load_model("mock").await.unwrap();

        let lease = runtime.model_lease("mock");
        let error = runtime.unload_model("mock").await.unwrap_err();
        assert_eq!(error.kind, AIError::ProviderUnavailable);
        assert_eq!(runtime.runtime_info("test").await.loaded_models.len(), 1);

        drop(lease);
        runtime.unload_model("mock").await.unwrap();
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
    }

    #[tokio::test]
    async fn registered_model_is_not_resident_until_loaded() {
        let runtime = Runtime::new();
        runtime.register(mock_spec()).await;
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
        runtime.load_model("mock").await.unwrap();
        let info = runtime.runtime_info("test").await;
        assert_eq!(info.loaded_models.len(), 1);
        assert_eq!(info.loaded_models[0].state, "ready");
        runtime.unload_model("mock").await.unwrap();
        assert!(runtime.runtime_info("test").await.loaded_models.is_empty());
    }

    #[test]
    fn exposes_mock_and_llama_descriptors() {
        let runtime = Runtime::new();
        let ids: Vec<_> = runtime
            .provider_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(ids, vec!["llama.cpp", "mock"]);
    }
}
