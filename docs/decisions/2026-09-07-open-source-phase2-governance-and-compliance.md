# 开源筹备阶段 2：社区治理规范、安全策略与第三方许可合规

- Status: `implemented`
- Date: 2026-09-07
- Deciders: MacAI 核心维护团队

## 背景与问题

随着 MacAI 开源进程推进，项目需要具备公开透明的社区协作规范、清晰的安全责任边界以及严谨的法律合规性：

1. **社区协作缺乏标准引导**：外部贡献者在提交 PR 或反馈 Issue 时缺乏统一的环境要求、测试命令、Runner 插件开发规范与协作准则；
2. **安全模型需正式声明**：MacAI 的架构核心为本机无鉴权 daemon，需明确向社区与用户传达单机安全假设、Runner 进程权限范围及私密漏洞披露途径；
3. **第三方技术与模型合规披露**：项目整合了众多优秀的开源引擎（llama.cpp, whisper.cpp, MLX 等）与库（如采用 MPL-2.0 的 symphonia），需清晰阐明授权边界与模型权重版权归属。

## 决策内容

### 1. 社区行为准则与贡献指南

- 落地标准 [Contributor Covenant v2.1](CODE_OF_CONDUCT.md)；
- 提供详尽的 [CONTRIBUTING.md](CONTRIBUTING.md)，明确本地环境需求、架构不变量、各子模块验证指令、如何为 `runners/` 贡献新插件，以及 Conventional Commits 提交规范。

### 2. 结构化 GitHub 协作模板

在 `.github/` 下配置标准模板：
- `ISSUE_TEMPLATE/config.yml`：禁用空白 Issue，提供安全私密提报和架构决策文档导航；
- `ISSUE_TEMPLATE/bug_report.yml`：基于 YAML Forms 的缺陷提报模板（收集 macOS 版本、芯片架构、组件分类与日志）；
- `ISSUE_TEMPLATE/feature_request.yml`：功能与新 Runner 提议模板；
- `pull_request_template.md`：PR 检查清单（测试覆盖、决策记录同步、依赖检查）。

### 3. 安全政策与威胁模型 (SECURITY.md)

正式声明：
- `aiworkd` 默认只绑定 `127.0.0.1:11435`，无身份鉴权，切勿直接暴露于公网；
- Runner 进程与 daemon 共享权限，非操作系统级沙箱；
- 音频与私密数据不持久化；
- 提供 GitHub Private Vulnerability Reporting 私密披露渠道。

### 4. 第三方许可证与模型免责声明 (THIRD_PARTY_LICENSES.md)

- 声明核心依赖与 Runner 引擎的开源协议；
- 特别针对入口纯 Rust 音频解码库 `symphonia` 披露其 **MPL-2.0** 许可证条款，确认未修改其源码，保持其余部分 MIT 属性；
- 明确模型免责声明：MacAI 仅为 Runtime 框架，不分发亦不拥有模型权重，用户使用具体模型需自行遵守各模型发布者的授权许可协议。

## 后果与验证

- 仓库具备完整的 GitHub Community Health Files 标准；
- 在 `README.md` 中增加社区、安全与许可入口；
- 相关文档与链接全部在本地通过静态校验。
