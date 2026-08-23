//! Provider 实现目录（文档 §8）。真实 backend 以独立进程接入，daemon
//! 中的 wrapper 负责生命周期、健康检查与协议转换。

pub mod llama_cpp;
pub mod mock;

pub use llama_cpp::LlamaCppProvider;
pub use mock::MockProvider;
