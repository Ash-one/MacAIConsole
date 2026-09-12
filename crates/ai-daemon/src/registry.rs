//! SQLite 持久化模型注册表。
//!
//! 数据库位于应用支持目录 `models.db`；当前字段契约由 ModelSpec、Model Profile v1
//! 与 Runner 插件架构决策共同拥有。
//! Runtime 启动时全量加载到内存 HashMap；此后每次 register / unregister /
//! 状态变更 / touch 都同步写库，内存 HashMap 仍是唯一读路径（单机 daemon，
//! 无并发进程访问该库）。

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use ai_core::model::ModelSpec;

pub struct RegistryStore {
    conn: Mutex<Connection>,
}

/// 表结构与 ModelSpec 字段一一对应；Profile catalog 与 per-model snapshot 分表保存。
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS models (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    provider TEXT NOT NULL,
    requested_provider TEXT,
    provider_selection_reason TEXT,
    source TEXT,
    path TEXT,
    format TEXT,
    size_bytes INTEGER,
    memory_estimate INTEGER,
    keep_alive TEXT,
    context_length INTEGER,
    state TEXT NOT NULL DEFAULT 'unloaded',
    installed_at INTEGER NOT NULL,
    last_used_at INTEGER,
    default_voice TEXT,
    temperature REAL,
    top_p REAL,
    max_tokens INTEGER
);
CREATE TABLE IF NOT EXISTS model_profiles (
    profile_id TEXT PRIMARY KEY,
    runner TEXT NOT NULL,
    compatibility TEXT NOT NULL,
    digest TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    installed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS registered_model_profiles (
    model_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    runner TEXT NOT NULL,
    compatibility TEXT NOT NULL,
    digest TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    registered_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS runner_packages (
    runner_id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    digest TEXT NOT NULL,
    installed_at INTEGER NOT NULL
);
";

/// 持久化的 Runner Model Profile snapshot。`digest` 是 profile 规范 JSON 的
/// sha256（`ModelProfile::digest`）；`snapshot` 是规范 JSON 本身，重启后可恢复
/// 解析，不与任何单个模型实例绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProfile {
    pub profile_id: String,
    pub runner: String,
    pub compatibility: String,
    pub digest: String,
    pub snapshot: String,
    pub installed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRunnerPackage {
    pub runner_id: String,
    pub version: String,
    pub digest: String,
    pub installed_at: u64,
}

impl RegistryStore {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create registry dir: {error}"))?;
        }
        let conn = Connection::open(db_path)
            .map_err(|error| format!("cannot open registry db: {error}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|error| format!("cannot init registry schema: {error}"))?;
        // 旧库迁移：default_voice 列后加，缺列时补上。
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN default_voice TEXT;");
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN requested_provider TEXT;");
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN provider_selection_reason TEXT;");
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN temperature REAL;");
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN top_p REAL;");
        let _ = conn.execute_batch("ALTER TABLE models ADD COLUMN max_tokens INTEGER;");
        let _ = conn.execute(
            "UPDATE models SET requested_provider = 'unknown'
             WHERE requested_provider IS NULL",
            [],
        );
        let _ = conn.execute(
            "UPDATE models SET provider_selection_reason =
                'registration predates provider selection audit metadata'
             WHERE provider_selection_reason IS NULL",
            [],
        );
        let _ = conn.execute(
            "UPDATE models SET provider = 'org.macai.whisper.cpp',
                provider_selection_reason = 'legacy provider alias ''whisper.cpp'' migrated to Runner'
             WHERE provider = 'whisper.cpp'",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn runner_packages(&self) -> Vec<StoredRunnerPackage> {
        let connection = self.conn.lock().expect("registry lock");
        let mut statement = match connection.prepare(
            "SELECT runner_id, version, digest, installed_at FROM runner_packages ORDER BY runner_id",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map([], |row| {
                Ok(StoredRunnerPackage {
                    runner_id: row.get(0)?,
                    version: row.get(1)?,
                    digest: row.get(2)?,
                    installed_at: row.get::<_, i64>(3)?.max(0) as u64,
                })
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn trust_runner_package(
        &self,
        runner_id: &str,
        version: &str,
        digest: &str,
        installed_at: u64,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .expect("registry lock")
            .execute(
                "INSERT INTO runner_packages (runner_id, version, digest, installed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(runner_id) DO UPDATE SET
                    version = excluded.version,
                    digest = excluded.digest,
                    installed_at = excluded.installed_at",
                rusqlite::params![runner_id, version, digest, installed_at as i64],
            )
            .map(|_| ())
            .map_err(|error| format!("cannot persist Runner package trust: {error}"))
    }

    /// 启动时全量加载。状态一律重置为 unloaded：daemon 重启后没有任何
    /// worker 进程存活，持久化的 ready 状态只是陈旧记录。
    pub fn load_all(&self) -> Vec<(ModelSpec, Option<u64>)> {
        let Ok(conn) = self.conn.lock() else {
            return vec![];
        };
        let mut stmt = match conn.prepare(
            "SELECT id, name, type, provider, requested_provider, provider_selection_reason,
                    source, path, format, size_bytes, memory_estimate, keep_alive,
                    context_length, last_used_at, default_voice, temperature, top_p, max_tokens
             FROM models ORDER BY id",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return vec![],
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<f64>>(15)?,
                row.get::<_, Option<f64>>(16)?,
                row.get::<_, Option<i64>>(17)?,
            ))
        });
        let mut result = Vec::new();
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let is_llm = row.2 == "llm";
                let spec = ModelSpec {
                    id: row.0,
                    name: row.1,
                    model_type: row.2,
                    provider: row.3,
                    requested_provider: row.4,
                    provider_selection_reason: row.5,
                    source: row.6,
                    path: row.7,
                    format: row.8,
                    size_bytes: row.9.map(|v| v.max(0) as u64),
                    memory_estimate: row.10.map(|v| v.max(0) as u64),
                    keep_alive: row.11,
                    context_length: row.12.map(|v| v.max(0) as u64),
                    temperature: row.15.or(is_llm.then_some(1.0)),
                    top_p: row.16.or(is_llm.then_some(0.95)),
                    max_tokens: row.17.map(|v| v.max(1) as u64).or(is_llm.then_some(1024)),
                    default_voice: row.14,
                };
                result.push((spec, row.13.map(|v| v.max(0) as u64)));
            }
        }
        // 重启后全部视为 unloaded，清掉陈旧的 loaded 时间戳语义由调用方处理。
        let _ = conn.execute("UPDATE models SET state = 'unloaded'", []);
        result
    }

    pub fn upsert(&self, spec: &ModelSpec, installed_at: u64, last_used_at: Option<u64>) {
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        let _ = conn.execute(
            "INSERT INTO models (id, name, type, provider, requested_provider,
                                 provider_selection_reason, source, path, format, size_bytes,
                                 memory_estimate, keep_alive, context_length, state,
                                 installed_at, last_used_at, default_voice, temperature, top_p, max_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     'unloaded', ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
                name = ?2, type = ?3, provider = ?4, requested_provider = ?5,
                provider_selection_reason = ?6, source = ?7, path = ?8,
                format = ?9, size_bytes = ?10, memory_estimate = ?11,
                keep_alive = ?12, context_length = ?13, default_voice = ?16,
                temperature = ?17, top_p = ?18, max_tokens = ?19",
            rusqlite::params![
                spec.id,
                spec.name,
                spec.model_type,
                spec.provider,
                spec.requested_provider,
                spec.provider_selection_reason,
                spec.source,
                spec.path,
                spec.format,
                spec.size_bytes.map(|v| v as i64),
                spec.memory_estimate.map(|v| v as i64),
                spec.keep_alive,
                spec.context_length.map(|v| v as i64),
                installed_at as i64,
                last_used_at.map(|v| v as i64),
                spec.default_voice,
                spec.temperature,
                spec.top_p,
                spec.max_tokens.map(|v| v as i64),
            ],
        );
    }

    pub fn remove(&self, id: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("DELETE FROM models WHERE id = ?1", [id]);
            let _ = conn.execute(
                "DELETE FROM registered_model_profiles WHERE model_id = ?1",
                [id],
            );
        }
    }

    pub fn set_state(&self, id: &str, state: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("UPDATE models SET state = ?2 WHERE id = ?1", [id, state]);
        }
    }

    /// 只更新 keep_alive 字段（运行状态页行内调整策略，不重载进程）。
    pub fn set_keep_alive(&self, id: &str, keep_alive: Option<&str>) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE models SET keep_alive = ?2 WHERE id = ?1",
                rusqlite::params![id, keep_alive],
            );
        }
    }

    /// 只更新 default_voice 字段（右键菜单切换 TTS 默认音色）。
    pub fn set_default_voice(&self, id: &str, voice: Option<&str>) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE models SET default_voice = ?2 WHERE id = ?1",
                rusqlite::params![id, voice],
            );
        }
    }

    pub fn set_generation_settings(&self, id: &str, temperature: f64, top_p: f64, max_tokens: u64) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE models SET temperature = ?2, top_p = ?3, max_tokens = ?4 WHERE id = ?1",
                rusqlite::params![id, temperature, top_p, max_tokens as i64],
            );
        }
    }

    pub fn touch(&self, id: &str, ts: u64) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE models SET last_used_at = ?2 WHERE id = ?1",
                rusqlite::params![id, ts as i64],
            );
        }
    }

    /// 持久化一个 Model Profile snapshot。同 profile_id 的新 digest 覆盖旧
    /// snapshot（profile 内容升级路径）；installed_at 属于首次注册时间，upsert
    /// 不覆盖。调用方负责先 `ModelProfile::validate`。
    pub fn upsert_profile(
        &self,
        profile_id: &str,
        runner: &str,
        compatibility: &str,
        digest: &str,
        snapshot: &str,
        installed_at: u64,
    ) {
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        let _ = conn.execute(
            "INSERT INTO model_profiles (profile_id, runner, compatibility, digest,
                                         snapshot, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(profile_id) DO UPDATE SET
                runner = ?2, compatibility = ?3, digest = ?4, snapshot = ?5",
            rusqlite::params![
                profile_id,
                runner,
                compatibility,
                digest,
                snapshot,
                installed_at as i64
            ],
        );
    }

    #[allow(dead_code)] // 精确的单条 Profile 查询 API；当前生产启动路径使用全量恢复。
    pub fn get_profile(&self, profile_id: &str) -> Option<StoredProfile> {
        let conn = self.conn.lock().ok()?;
        conn.query_row(
            "SELECT profile_id, runner, compatibility, digest, snapshot, installed_at
             FROM model_profiles WHERE profile_id = ?1",
            [profile_id],
            |row| {
                Ok(StoredProfile {
                    profile_id: row.get(0)?,
                    runner: row.get(1)?,
                    compatibility: row.get(2)?,
                    digest: row.get(3)?,
                    snapshot: row.get(4)?,
                    installed_at: row.get::<_, i64>(5)?.max(0) as u64,
                })
            },
        )
        .ok()
    }

    /// 启动时全量加载持久化 profiles（重启恢复路径）。
    #[allow(dead_code)] // 当前供验证 catalog 持久化；生产组合直接维护内存 catalog。
    pub fn load_profiles(&self) -> Vec<StoredProfile> {
        let Ok(conn) = self.conn.lock() else {
            return vec![];
        };
        let mut stmt = match conn.prepare(
            "SELECT profile_id, runner, compatibility, digest, snapshot, installed_at
             FROM model_profiles ORDER BY profile_id",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return vec![],
        };
        let rows = stmt.query_map([], |row| {
            Ok(StoredProfile {
                profile_id: row.get(0)?,
                runner: row.get(1)?,
                compatibility: row.get(2)?,
                digest: row.get(3)?,
                snapshot: row.get(4)?,
                installed_at: row.get::<_, i64>(5)?.max(0) as u64,
            })
        });
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => vec![],
        }
    }

    /// 将模型注册绑定到当时经过校验的 immutable Profile snapshot。后续 bundled
    /// catalog 更新不会覆盖这里的 per-model authority。
    pub fn bind_model_profile(&self, model_id: &str, profile: &StoredProfile) {
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        let _ = conn.execute(
            "INSERT INTO registered_model_profiles
                (model_id, profile_id, runner, compatibility, digest, snapshot, registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(model_id) DO UPDATE SET
                profile_id = ?2, runner = ?3, compatibility = ?4,
                digest = ?5, snapshot = ?6",
            rusqlite::params![
                model_id,
                profile.profile_id,
                profile.runner,
                profile.compatibility,
                profile.digest,
                profile.snapshot,
                profile.installed_at as i64,
            ],
        );
    }

    pub fn load_model_profile_bindings(&self) -> std::collections::HashMap<String, StoredProfile> {
        let Ok(conn) = self.conn.lock() else {
            return std::collections::HashMap::new();
        };
        let mut stmt = match conn.prepare(
            "SELECT model_id, profile_id, runner, compatibility, digest, snapshot, registered_at
             FROM registered_model_profiles ORDER BY model_id",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return std::collections::HashMap::new(),
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                StoredProfile {
                    profile_id: row.get(1)?,
                    runner: row.get(2)?,
                    compatibility: row.get(3)?,
                    digest: row.get(4)?,
                    snapshot: row.get(5)?,
                    installed_at: row.get::<_, i64>(6)?.max(0) as u64,
                },
            ))
        });
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_db(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "macai-registry-test-{}-{}-{}.db",
            label,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn spec(id: &str) -> ModelSpec {
        ModelSpec {
            id: id.to_string(),
            name: format!("{id} name"),
            model_type: "llm".to_string(),
            provider: "llama.cpp".to_string(),
            requested_provider: Some("llama.cpp".to_string()),
            provider_selection_reason: Some("explicit provider selection".to_string()),
            source: Some("hf:test/repo".to_string()),
            path: Some(format!("/Models/llm/{id}.gguf")),
            format: Some("gguf".to_string()),
            size_bytes: Some(1024),
            memory_estimate: Some(2048),
            keep_alive: Some("5m".to_string()),
            context_length: Some(4096),
            temperature: Some(1.0),
            top_p: Some(0.95),
            max_tokens: Some(1024),
            default_voice: None,
        }
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// 绕过 store 直接读库，观察持久化结果而不是内存缓存。
    fn persisted(path: &Path, sql: &str, id: &str) -> Option<String> {
        let conn = Connection::open(path).ok()?;
        let value: rusqlite::types::Value = conn.query_row(sql, [id], |row| row.get(0)).ok()?;
        Some(match value {
            rusqlite::types::Value::Integer(i) => i.to_string(),
            rusqlite::types::Value::Real(f) => f.to_string(),
            rusqlite::types::Value::Text(s) => s,
            _ => return None,
        })
    }

    #[test]
    fn upsert_then_load_roundtrips_every_spec_field() {
        let path = temp_db("roundtrip");
        let store = RegistryStore::open(&path).unwrap();
        let mut s = spec("qwen3");
        s.default_voice = Some("zf_001".to_string());
        store.upsert(&s, 111, Some(222));

        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        let (loaded_spec, last_used_at) = &loaded[0];
        assert_eq!(loaded_spec.id, "qwen3");
        assert_eq!(loaded_spec.name, "qwen3 name");
        assert_eq!(loaded_spec.model_type, "llm");
        assert_eq!(loaded_spec.provider, "llama.cpp");
        assert_eq!(loaded_spec.requested_provider.as_deref(), Some("llama.cpp"));
        assert_eq!(
            loaded_spec.provider_selection_reason.as_deref(),
            Some("explicit provider selection")
        );
        assert_eq!(loaded_spec.source.as_deref(), Some("hf:test/repo"));
        assert_eq!(loaded_spec.path.as_deref(), Some("/Models/llm/qwen3.gguf"));
        assert_eq!(loaded_spec.format.as_deref(), Some("gguf"));
        assert_eq!(loaded_spec.size_bytes, Some(1024));
        assert_eq!(loaded_spec.temperature, Some(1.0));
        assert_eq!(loaded_spec.top_p, Some(0.95));
        assert_eq!(loaded_spec.max_tokens, Some(1024));
        assert_eq!(loaded_spec.memory_estimate, Some(2048));
        assert_eq!(loaded_spec.keep_alive.as_deref(), Some("5m"));
        assert_eq!(loaded_spec.context_length, Some(4096));
        assert_eq!(loaded_spec.default_voice.as_deref(), Some("zf_001"));
        assert_eq!(*last_used_at, Some(222));
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn opening_legacy_database_marks_selection_history_unknown() {
        let path = temp_db("selection-migration");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE models (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                type TEXT NOT NULL,
                provider TEXT NOT NULL,
                source TEXT,
                path TEXT,
                format TEXT,
                size_bytes INTEGER,
                memory_estimate INTEGER,
                keep_alive TEXT,
                context_length INTEGER,
                state TEXT NOT NULL DEFAULT 'unloaded',
                installed_at INTEGER NOT NULL,
                last_used_at INTEGER,
                default_voice TEXT
             );
             INSERT INTO models (id, name, type, provider, state, installed_at)
             VALUES ('legacy', 'Legacy', 'llm', 'llama.cpp', 'unloaded', 1);
             INSERT INTO models (id, name, type, provider, state, installed_at)
             VALUES ('whisper', 'Whisper', 'stt', 'whisper.cpp', 'unloaded', 1);",
        )
        .unwrap();
        drop(conn);

        let store = RegistryStore::open(&path).unwrap();
        let loaded = store.load_all();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].0.requested_provider.as_deref(), Some("unknown"));
        assert_eq!(
            loaded[0].0.provider_selection_reason.as_deref(),
            Some("registration predates provider selection audit metadata")
        );
        assert_eq!(loaded[1].0.provider, "org.macai.whisper.cpp");
        assert_eq!(
            loaded[1].0.provider_selection_reason.as_deref(),
            Some("legacy provider alias 'whisper.cpp' migrated to Runner")
        );
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn load_all_resets_persisted_state_to_unloaded() {
        // daemon 重启后没有任何 worker 进程存活，持久化的 ready 只是陈旧记录。
        let path = temp_db("state-reset");
        let store = RegistryStore::open(&path).unwrap();
        store.upsert(&spec("m"), 100, None);
        store.set_state("m", "ready");
        assert_eq!(
            persisted(&path, "SELECT state FROM models WHERE id = ?1", "m").as_deref(),
            Some("ready")
        );

        store.load_all();
        assert_eq!(
            persisted(&path, "SELECT state FROM models WHERE id = ?1", "m").as_deref(),
            Some("unloaded")
        );
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn runner_package_trust_roundtrips() {
        let path = temp_db("runner-packages");
        let store = RegistryStore::open(&path).unwrap();
        store
            .trust_runner_package("org.example.echo", "0.1.0", "digest-a", 123)
            .unwrap();
        assert_eq!(
            store.runner_packages(),
            vec![StoredRunnerPackage {
                runner_id: "org.example.echo".to_string(),
                version: "0.1.0".to_string(),
                digest: "digest-a".to_string(),
                installed_at: 123,
            }]
        );
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn upsert_conflict_updates_fields_but_preserves_timestamps() {
        let path = temp_db("conflict");
        let store = RegistryStore::open(&path).unwrap();
        store.upsert(&spec("m"), 111, Some(222));

        let mut renamed = spec("m");
        renamed.name = "renamed".to_string();
        renamed.keep_alive = Some("always".to_string());
        store.upsert(&renamed, 999, Some(888));

        let (loaded_spec, last_used_at) = &store.load_all()[0];
        assert_eq!(loaded_spec.name, "renamed");
        assert_eq!(loaded_spec.keep_alive.as_deref(), Some("always"));
        // installed_at / last_used_at 只属于首次安装，重新 upsert 不能覆盖。
        assert_eq!(
            persisted(&path, "SELECT installed_at FROM models WHERE id = ?1", "m").as_deref(),
            Some("111")
        );
        assert_eq!(*last_used_at, Some(222));
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn partial_updates_and_remove_touch_only_the_target_row() {
        let path = temp_db("partial");
        let store = RegistryStore::open(&path).unwrap();
        store.upsert(&spec("a"), 1, None);
        store.upsert(&spec("b"), 1, None);

        store.set_state("a", "failed");
        assert_eq!(
            persisted(&path, "SELECT state FROM models WHERE id = ?1", "a").as_deref(),
            Some("failed")
        );
        assert_eq!(
            persisted(&path, "SELECT state FROM models WHERE id = ?1", "b").as_deref(),
            Some("unloaded")
        );

        store.set_keep_alive("a", None);
        store.set_default_voice("a", Some("zf_001"));
        store.set_generation_settings("a", 0.6, 0.8, 256);
        store.touch("a", 777);
        let loaded: Vec<(ModelSpec, Option<u64>)> = store
            .load_all()
            .into_iter()
            .filter(|(spec, _)| spec.id == "a")
            .collect();
        assert_eq!(loaded[0].0.temperature, Some(0.6));
        assert_eq!(loaded[0].0.top_p, Some(0.8));
        assert_eq!(loaded[0].0.max_tokens, Some(256));
        assert_eq!(loaded[0].0.keep_alive, None);
        assert_eq!(loaded[0].0.default_voice.as_deref(), Some("zf_001"));
        assert_eq!(loaded[0].1, Some(777));

        store.remove("a");
        let ids: Vec<String> = store
            .load_all()
            .into_iter()
            .map(|(spec, _)| spec.id)
            .collect();
        assert_eq!(ids, vec!["b".to_string()]);
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn profile_snapshot_roundtrips_and_digest_conflict_updates() {
        let path = temp_db("profiles");
        let store = RegistryStore::open(&path).unwrap();
        store.upsert_profile(
            "kokoro-82m-zh",
            "org.macai.kokoro",
            ">=0.1,<0.2",
            "digest-a",
            r#"{"schema":"macai.model.v1"}"#,
            111,
        );
        let loaded = store.get_profile("kokoro-82m-zh").expect("profile stored");
        assert_eq!(loaded.runner, "org.macai.kokoro");
        assert_eq!(loaded.compatibility, ">=0.1,<0.2");
        assert_eq!(loaded.digest, "digest-a");
        assert_eq!(loaded.installed_at, 111);

        store.upsert(&spec("runner-model"), 111, None);
        store.bind_model_profile("runner-model", &loaded);
        let bindings = store.load_model_profile_bindings();
        assert_eq!(bindings["runner-model"], loaded);

        // 同 id 新 digest 覆盖内容，但 installed_at 保持首次注册值。
        store.upsert_profile(
            "kokoro-82m-zh",
            "org.macai.kokoro",
            ">=0.1,<0.3",
            "digest-b",
            r#"{"schema":"macai.model.v1","v":2}"#,
            999,
        );
        let updated = store.get_profile("kokoro-82m-zh").unwrap();
        assert_eq!(updated.digest, "digest-b");
        assert_eq!(updated.compatibility, ">=0.1,<0.3");
        assert_eq!(
            updated.installed_at, 111,
            "installed_at must be first-registration"
        );
        assert_eq!(
            store.load_model_profile_bindings()["runner-model"].digest,
            "digest-a",
            "catalog updates must not rewrite a registered model snapshot"
        );

        store.remove("runner-model");
        assert!(store.load_model_profile_bindings().is_empty());

        assert!(store.get_profile("missing").is_none());
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn profiles_survive_store_reopen() {
        let path = temp_db("profiles-reopen");
        {
            let store = RegistryStore::open(&path).unwrap();
            store.upsert_profile(
                "fake-tts",
                "org.example.fake",
                ">=0.1,<0.2",
                "digest-1",
                r#"{"id":"fake-tts"}"#,
                5,
            );
        }
        let reopened = RegistryStore::open(&path).unwrap();
        let profiles = reopened.load_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].profile_id, "fake-tts");
        assert_eq!(profiles[0].digest, "digest-1");
        assert_eq!(profiles[0].installed_at, 5);
        drop(reopened);
        cleanup(&path);
    }
}
