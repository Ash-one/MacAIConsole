//! 内存预算与模型调度（handoff §22–§26）。
//!
//! - Budget：`min(ram * 0.75, ram - 8GB)`，`AIWORKD_MEMORY_BUDGET` 可覆盖（字节）。
//! - LRU：加载前检查预算，不足时按 last_used 升序逐出无 lease 的 idle 模型。
//! - Keep-alive reaper：后台周期扫描，到期且空闲的常驻模型自动卸载。

use sysinfo::System;

/// 解析 keep_alive 字符串为秒；None 表示永不自动卸载（"always" / 无法解析时从宽处理）。
pub fn parse_keep_alive(value: Option<&str>) -> Option<u64> {
    let raw = value.map(str::trim)?;
    if raw.is_empty() || raw == "always" || raw == "-1" {
        return None;
    }
    if raw == "0" {
        return Some(0);
    }
    // 纯数字：按秒解析
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    // 带单位后缀：s/m/h/d
    let (num, unit) = raw.split_at(raw.len() - 1);
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };
    num.trim().parse::<u64>().ok().map(|n| n * multiplier)
}

/// 物理内存字节数。读取失败返回 None，调用方跳过预算约束。
pub fn total_memory_bytes() -> Option<u64> {
    let mut system = System::new();
    system.refresh_memory();
    let total = system.total_memory();
    (total > 0).then_some(total)
}

/// 当前已使用的物理内存字节数（供 GUI 内存压力条展示）。
pub fn used_memory_bytes() -> Option<u64> {
    let mut system = System::new();
    system.refresh_memory();
    let used = system.used_memory();
    (used > 0).then_some(used)
}

/// AI 内存预算（handoff §23）。环境变量 `AIWORKD_MEMORY_BUDGET`（字节）优先。
pub fn memory_budget() -> Option<u64> {
    if let Ok(raw) = std::env::var("AIWORKD_MEMORY_BUDGET") {
        if let Ok(bytes) = raw.trim().parse::<u64>() {
            return Some(bytes);
        }
    }
    total_memory_bytes()
        .map(|total| (total * 3 / 4).min(total.saturating_sub(8 * 1024 * 1024 * 1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_alive_parsing() {
        assert_eq!(parse_keep_alive(Some("always")), None);
        assert_eq!(parse_keep_alive(None), None);
        assert_eq!(parse_keep_alive(Some("")), None);
        assert_eq!(parse_keep_alive(Some("0")), Some(0));
        assert_eq!(parse_keep_alive(Some("5m")), Some(300));
        assert_eq!(parse_keep_alive(Some("30m")), Some(1800));
        assert_eq!(parse_keep_alive(Some("2h")), Some(7200));
        assert_eq!(parse_keep_alive(Some("90")), Some(90));
        assert_eq!(parse_keep_alive(Some("1d")), Some(86400));
    }

    #[test]
    fn keep_alive_parsing_trims_whitespace_and_rejects_malformed_values() {
        // 来自 GUI/设置文件的值可能有首尾空白；无法解析时从宽处理为 always。
        assert_eq!(parse_keep_alive(Some("  always  ")), None);
        assert_eq!(parse_keep_alive(Some(" 30m ")), Some(1800));
        assert_eq!(parse_keep_alive(Some("-1")), None);
        assert_eq!(parse_keep_alive(Some("10s")), Some(10));
        assert_eq!(parse_keep_alive(Some("0s")), Some(0));
        assert_eq!(parse_keep_alive(Some("5x")), None);
        assert_eq!(parse_keep_alive(Some("1.5h")), None);
        assert_eq!(parse_keep_alive(Some("m")), None);
    }
}
