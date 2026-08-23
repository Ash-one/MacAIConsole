//! Provider 实现目录（文档 §8）。第一阶段只含 Mock；真实 backend
//! （llama.cpp / MLX / whisper.cpp / MLX-Audio）在后续 milestone 作为
//! 独立进程接入，daemon 里对应的是进程管理 wrapper。

pub mod mock;

pub use mock::MockProvider;
