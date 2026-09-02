//! Runner discovery、manifest/Profile、协议和受监督子进程的通用基础。
//!
//! 这里不替换既有 Provider 路径。Kokoro cutover 只能在真实 Runner 通过完整行为
//! 验收后进行；本模块的职责是为该迁移提供可独立验证的边界。

mod environment;
mod manifest;
mod profile;
mod protocol;
mod registry;
mod supervisor;

pub use environment::{EnvironmentFingerprint, EnvironmentInput};
pub use manifest::{ManifestError, RunnerManifest, RunnerRuntime};
pub use profile::{ModelProfile, ProfileError};
pub use protocol::{
    read_frame, write_frame, Envelope, ProtocolError, DEFAULT_MAX_FRAME_BYTES, RUNNER_PROTOCOL_V1,
};
pub use registry::{RunnerDescriptor, RunnerRegistry, RunnerState};
pub use supervisor::{RunnerProcess, SupervisorError};
