## 改动说明

简要描述本次 PR 解决的问题或新增的特性。

关联 Issue: Fixes #

## 改动类型

- [ ] 缺陷修复 (Bug fix)
- [ ] 新功能 (New feature)
- [ ] 新 Runner 引擎支持 (New Runner)
- [ ] 性能优化 (Performance improvement)
- [ ] 架构/重构 (Refactoring)
- [ ] 文档更新 (Documentation update)

## 验证与测试

请说明您在本地执行了哪些验证，并确认勾选：

- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo test --workspace` 通过
- [ ] `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest`（若涉及 GUI 代码）通过
- [ ] Runner package 测试（若涉及 Python / Runner 代码）通过
- [ ] 若改动涉及对外接口、行为或共享契约，已在 `docs/decisions/` 新增或更新对应的 Decision Record

## 检查清单

- [ ] 代码遵循本仓库的设计哲学（单一 Authority、进程级隔离、无静默回退）
- [ ] 没有引入不必要的庞大外部二进制依赖
- [ ] 没有提交个人本地绝对路径或敏感凭据
