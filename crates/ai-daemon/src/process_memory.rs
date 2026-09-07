//! 针对常驻进程与 Runner 实例的物理内存采样。
//!
//! 在 macOS (Apple Silicon) 上，通过 `sysinfo` 获取对应进程当前驻留的物理内存（RSS）。
//! 对于产生子进程的 Runner 架构（如 llama.cpp / whisper.cpp 的 Python wrapper 启动原生 server），
//! 会递归累加根进程及其所有子孙进程的物理内存，避免漏算核心推理引擎的内存开销。
//! 采样完全在 OS 层进行，不获取 Runner I/O 锁，不阻塞推理。

use std::collections::HashSet;
use sysinfo::{Pid, ProcessesToUpdate, System};

/// 读取指定根进程及其所有子孙进程的物理驻留内存总和（字节）。
///
/// 若进程不存在或未驻留物理内存，返回 `None`。
pub fn resident_memory_bytes(root_pid: u32) -> Option<u64> {
    let mut system = System::new();
    let root = Pid::from_u32(root_pid);
    system.refresh_processes(ProcessesToUpdate::All, true);

    let root_process = system.process(root)?;
    let mut total = root_process.memory();

    let mut target_pids = HashSet::new();
    target_pids.insert(root);

    // 递归累加所有以 root_pid 为祖先的子孙进程
    let mut added = true;
    while added {
        added = false;
        for (pid, process) in system.processes() {
            if let Some(parent) = process.parent() {
                if target_pids.contains(&parent) && target_pids.insert(*pid) {
                    total = total.saturating_add(process.memory());
                    added = true;
                }
            }
        }
    }

    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_process_memory() {
        let pid = std::process::id();
        let memory = resident_memory_bytes(pid);
        assert!(
            memory.is_some(),
            "current process memory should be readable"
        );
        assert!(
            memory.unwrap() > 0,
            "current process memory should be greater than 0"
        );
    }

    #[test]
    fn test_nonexistent_process_memory() {
        // u32::MAX is practically guaranteed not to be a valid running PID
        let memory = resident_memory_bytes(u32::MAX);
        assert_eq!(memory, None);
    }
}
