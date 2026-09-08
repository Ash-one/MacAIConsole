# GUI 公开远端模型下载

Status: implemented

Class: behavior

Owner: this file

## Problem

推荐模型下载依赖 bundled Model Profile 的固定 artifact 清单；其他公开模型只能由用户
先在浏览器或命令行下载，再通过 GUI 导入。GUI 缺少从 Hugging Face 或 ModelScope
仓库 ID 读取文件清单并下载到统一模型仓库的入口。

## Decision

- `aiworkd` 新增 `POST /api/models/remote/inspect`，根据显式平台与 `owner/repo`
  获取普通文件路径、大小和当前 revision；Hugging Face tree API 完整跟随同源分页，
  ModelScope 使用其递归 repo files API。
- `POST /api/models/pull` 增加可选 `source` 与 `revision`。缺省保持 Hugging Face
  `main` 行为；ModelScope 固定使用官方模型端点。Profile 下载同时传递已有 immutable
  revision。
- GUI 的「添加模型」窗口提供「在线仓库」模式。用户显式选择平台和 LLM/STT/TTS，
  预览文件与总大小后提交下载；完整模型页面 URL 仅用于解析平台和仓库 ID。
- LLM GGUF 与 Whisper `ggml-*.bin` 使用单文件选择并落到类型根目录；其他模型按
  `Models/<type>/<owner>--<repo>/` 保存并保留相对路径。
- 远端下载固定 `auto_load=false`。下载成功只刷新本地仓库，不注册、加载或推断
  Provider，也不代表当前 Runner 能运行该模型。
- 首版只支持公开仓库；不保存或传输 Hugging Face/ModelScope token。

## Safety and failure semantics

仓库 ID、revision 与文件路径沿用 daemon 的路径穿越校验；远端来源是固定枚举，调用方
不能提供任意下载主机。Hugging Face 分页只接受同源 next URL，文件清单上限 10,000。
远端查询或下载失败统一返回 `download_failed`，底层 HTTP 状态与原因保留在消息中。

## Verification

- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过（真实模型/引擎
  smoke 3 项按环境约束保持 ignored）；
- MacAIConsole 全量 43 项 Swift 测试通过，覆盖 ID/URL 解析、单文件识别和
  inspect/pull payload；
- 隔离临时 daemon 分别读取 `hf-internal-testing/tiny-random-gpt2` 与
  `Qwen/Qwen2.5-0.5B-Instruct` 的 Hugging Face/ModelScope 文件清单和 revision，
  并从两侧各下载 `config.json` 到对应 `owner--repo` 目录；响应均为
  `state=downloaded`，注册表未写入模型。
