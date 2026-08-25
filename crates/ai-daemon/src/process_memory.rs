use sysinfo::{Pid, ProcessesToUpdate, System};

/// 读取 worker 进程的 resident memory（字节）。
/// macOS 上对应进程当前驻留的物理内存，包含实际驻留的模型页与运行时分配。
pub(crate) fn resident_memory_bytes(pid: u32) -> Option<u64> {
    let mut system = System::new();
    let process_id = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).map(|process| process.memory())
}
