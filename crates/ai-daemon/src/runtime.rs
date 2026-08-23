//! Runtime — 文档 §6 的核心对象。
//!
//! 原型阶段实现：
//! - ModelRegistry（内存版，Milestone 2 换 SQLite）
//! - ProviderRegistry（内置 MockProvider）
//! - Scheduler（简化：模型缺失即报错，不自动加载）
//! - MemoryManager 占位（字段保留，逻辑在 Milestone 2/6 实现）

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

use ai_core::model::ModelSpec;
use ai_core::response::{LoadedModelInfo, RuntimeInfo};

use crate::providers::MockProvider;

/// 内存版模型注册表条目：模型规格 + 当前状态 + 使用时间。
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub spec: ModelSpec,
    pub state: String,
    pub loaded_at: Option<u64>,
    pub last_used_at: Option<u64>,
}

pub struct Runtime {
    /// 注册表：id → entry。
    registry: RwLock<HashMap<String, RegistryEntry>>,
    /// Provider 注册表：provider_id → Box<dyn Provider>。
    /// 原型只有 Mock；真实 backend 以进程形式接入（文档 §8）。
    chat_provider: Arc<MockProvider>,
    started_at: Instant,
    active_requests: AtomicU64,
    request_counter: AtomicU64,
}

/// 活跃请求计数守卫。普通请求返回、流式响应结束或客户端断开时自动归零。
pub struct RequestGuard {
    runtime: Arc<Runtime>,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.runtime.active_requests.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            registry: RwLock::new(HashMap::new()),
            chat_provider: Arc::new(MockProvider),
            started_at: Instant::now(),
            active_requests: AtomicU64::new(0),
            request_counter: AtomicU64::new(0),
        }
    }

    /// 注册一个模型（`ai pull` / 启动时内置模型）。
    pub async fn register(&self, spec: ModelSpec) {
        let mut reg = self.registry.write().await;
        reg.insert(
            spec.id.clone(),
            RegistryEntry {
                spec,
                state: "idle".to_string(),
                loaded_at: None,
                last_used_at: None,
            },
        );
    }

    /// 查询模型规格。
    pub async fn get_model(&self, id: &str) -> Option<ModelSpec> {
        self.registry.read().await.get(id).map(|e| e.spec.clone())
    }

    /// 全部模型（按 id 排序，输出稳定）。
    pub async fn list_models(&self) -> Vec<RegistryEntry> {
        let reg = self.registry.read().await;
        let mut v: Vec<RegistryEntry> = reg.values().cloned().collect();
        v.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
        v
    }

    /// 生成 request_id（文档 §44：每个 request 必须具备 request_id）。
    pub fn next_request_id(&self) -> String {
        let n = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("req_{}_{}", t, n)
    }

    /// 访问 Chat Provider（原型只有一个 Mock）。
    pub fn chat_provider(&self) -> Arc<MockProvider> {
        self.chat_provider.clone()
    }

    /// 标记一次请求开始；返回值必须持有到请求真正结束。
    pub fn request_guard(self: &Arc<Self>) -> RequestGuard {
        self.active_requests.fetch_add(1, Ordering::SeqCst);
        RequestGuard {
            runtime: Arc::clone(self),
        }
    }

    /// GET /api/runtime（文档 §17、§37）。
    pub async fn runtime_info(&self, version: &str) -> RuntimeInfo {
        let uptime = self.started_at.elapsed().as_secs();
        let loaded = self.list_models().await;
        let loaded_models = loaded
            .into_iter()
            .filter(|e| e.state != "unloaded")
            .map(|e| LoadedModelInfo {
                id: e.spec.id,
                provider: e.spec.provider.clone(),
                state: e.state,
                memory_estimate: e.spec.memory_estimate,
                keep_alive: e.spec.keep_alive.clone(),
                loaded_at: e.loaded_at,
                last_used_at: e.last_used_at,
            })
            .collect();
        RuntimeInfo {
            version: version.to_string(),
            pid: std::process::id(),
            uptime_secs: uptime,
            loaded_models,
            active_requests: self.active_requests.load(Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_guard_tracks_and_releases_active_request() {
        let runtime = Arc::new(Runtime::new());
        assert_eq!(runtime.runtime_info("test").await.active_requests, 0);

        let guard = runtime.request_guard();
        assert_eq!(runtime.runtime_info("test").await.active_requests, 1);

        drop(guard);
        assert_eq!(runtime.runtime_info("test").await.active_requests, 0);
    }
}
