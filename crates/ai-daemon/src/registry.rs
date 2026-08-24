//! SQLite 持久化模型注册表（handoff §19）。
//!
//! 数据库位于应用支持目录 `models.db`，schema 按 handoff 推荐定义。
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

/// 表结构与 ModelSpec 字段一一对应；context_length/keep_alive 存 parameters JSON。
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS models (
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
    last_used_at INTEGER
);
";

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
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 启动时全量加载。状态一律重置为 unloaded：daemon 重启后没有任何
    /// worker 进程存活，持久化的 ready 状态只是陈旧记录。
    pub fn load_all(&self) -> Vec<(ModelSpec, Option<u64>)> {
        let Ok(conn) = self.conn.lock() else {
            return vec![];
        };
        let mut stmt = match conn.prepare(
            "SELECT id, name, type, provider, source, path, format, size_bytes,
                    memory_estimate, keep_alive, context_length, last_used_at
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
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
            ))
        });
        let mut result = Vec::new();
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let spec = ModelSpec {
                    id: row.0,
                    name: row.1,
                    model_type: row.2,
                    provider: row.3,
                    source: row.4,
                    path: row.5,
                    format: row.6,
                    size_bytes: row.7.map(|v| v.max(0) as u64),
                    memory_estimate: row.8.map(|v| v.max(0) as u64),
                    keep_alive: row.9,
                    context_length: row.10.map(|v| v.max(0) as u64),
                };
                result.push((spec, row.11.map(|v| v.max(0) as u64)));
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
            "INSERT INTO models (id, name, type, provider, source, path, format,
                                 size_bytes, memory_estimate, keep_alive, context_length,
                                 state, installed_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'unloaded', ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                name = ?2, type = ?3, provider = ?4, source = ?5, path = ?6,
                format = ?7, size_bytes = ?8, memory_estimate = ?9,
                keep_alive = ?10, context_length = ?11",
            rusqlite::params![
                spec.id,
                spec.name,
                spec.model_type,
                spec.provider,
                spec.source,
                spec.path,
                spec.format,
                spec.size_bytes.map(|v| v as i64),
                spec.memory_estimate.map(|v| v as i64),
                spec.keep_alive,
                spec.context_length.map(|v| v as i64),
                installed_at as i64,
                last_used_at.map(|v| v as i64),
            ],
        );
    }

    pub fn remove(&self, id: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("DELETE FROM models WHERE id = ?1", [id]);
        }
    }

    pub fn set_state(&self, id: &str, state: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("UPDATE models SET state = ?2 WHERE id = ?1", [id, state]);
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
}
