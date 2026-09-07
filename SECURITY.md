# 安全政策 (Security Policy)

## 支持的版本

当前只有主分支（`main`）以及最新发布的稳定版本会接收安全修复补丁。

| 版本 | 支持状态 |
| :--- | :--- |
| main | :white_check_mark: 支持 |
| 0.1.x | :white_check_mark: 支持 |
| < 0.1.0 | :x: 不再支持 |

---

## 核心安全模型与假设

在使用 MacAI 时，请充分理解本项目的安全边界与设计假设：

1. **守护进程无鉴权（No-Auth Localhost Daemon）**：
   - 守护进程 `aiworkd` 默认且**仅绑定**本地回环地址 `127.0.0.1:11435`；
   - 本地 HTTP API（OpenAI-compatible endpoints 及管理接口）未设计认证鉴权机制；
   - **安全警告**：切勿将 `11435` 端口直接暴露到不受信的局域网或公网。如需远程访问，必须配合 SSH 隧道（SSH Tunnel）、Tailscale/VPN，或在前端部署具备严格身份认证（如 Bearer Token / mTLS）的反向代理（如 Nginx / Caddy）。

2. **Runner 进程权限与沙箱边界**：
   - 推理引擎（如 `llama.cpp`、`whisper.cpp`、Python MLX runners）均作为子进程运行，具有与运行 `aiworkd` 的系统用户相同的执行权限；
   - 尽管 daemon 会对 Runner manifest digest、环境变量白名单和输出路径进行基础校验，但这**不构成操作系统级别的安全沙箱**；
   - 仅使用由项目官方或受信任源提供的 Runner 配置与代码，切勿执行来源不明的 Runner 插件。

3. **数据隐私与脱敏**：
   - MacAI 专为本地运行设计，除用户主动触发的模型下载（默认从 Hugging Face / ModelScope）之外，所有推理、文本交互和语音转写均在本地完成，无任何数据遥测（Telemetry）或上传；
   - 语音转写（STT）上传的原始音频由 daemon 在入口完成 PCM WAV 转换并按流式处理，**不会写入**持久化任务历史；
   - 日志系统严格过滤 API Token 与模型上下文内容。

---

## 报告安全漏洞

如果您在 MacAI 中发现了安全漏洞，请不要通过公开的 GitHub Issue 进行报告。

请通过以下方式提交私密漏洞报告：

1. **GitHub Private Vulnerability Reporting**（推荐）：
   - 访问仓库的 **Security** 标签页；
   - 点击 **Report a vulnerability** 开启私密披露对话。
2. **安全邮件**：
   - 如果私密提报功能不可用，请通过 GitHub Profile 中的维护者联系邮箱发送邮件，并在主题中注明 `[SECURITY] MacAI Vulnerability Report`。

请在报告中包含：
- 影响的组件（`aiworkd`、`macai` CLI、`MacAIConsole` 或特定 Runner）；
- 漏洞类型与复现步骤（包括最小可复现示例 PoC）；
- 潜在的危害与影响范围；
- 如果已知，提供建议的修复方案或补丁。

我们将尽快确认收到报告，并在验证后协调修复方案与负责任的披露时间表。
