//! Provider 实现目录（文档 §8）。真实 backend 以独立进程接入，daemon
//! 中的 wrapper 负责生命周期、健康检查与协议转换。

pub mod kokoro_mlx;
pub mod llama_cpp;
pub mod macos_say;
pub mod mock;
pub mod whisper_cpp;

pub use kokoro_mlx::KokoroMlxProvider;
pub use llama_cpp::LlamaCppProvider;
pub use macos_say::MacOSSayProvider;
pub use mock::MockProvider;
pub use whisper_cpp::WhisperCppProvider;
