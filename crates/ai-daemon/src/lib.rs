//! 可复用的 aiworkd daemon 组件。
//!
//! 二进制入口保留在 `main.rs`；Runner foundation 放在 library 中，供内置 Runner、
//! 集成测试和后续 daemon composition 共用。

pub mod process_memory;
pub mod runners;
