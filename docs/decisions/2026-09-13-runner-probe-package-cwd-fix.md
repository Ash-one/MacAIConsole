# Decision: Runner 环境 probe 的工作目录对齐 package root

Status: implemented

Class: fix

Owner: this file

Related current decisions: [uv 管理全部 Python 环境](2026-09-02-uv-python-environments.md)、[单文件 Script Runner 创建](2026-09-08-single-file-script-runner.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)

Contract: [runner-manifest-v1.md](../specs/runner-manifest-v1.md) Environment probe 节

## Problem

单文件 Script Runner 走完 create（staging 内 uv sync + probe 通过、digest trust 落盘）
并重启装配后，GUI 的引擎环境安装（`ensure_environment`）必然失败。2026-09-13 真实日志
（`org.hojoai.hojo-tts-python`）：

```text
environment probe failed: probe exited with exit status: 2:
.../Runtimes/python/org.hojoai.hojo-tts-python/installing/probe-runtime/script_host.py:
[Errno 2] No such file or directory
```

根因是两段代码对 probe 相对参数的语义空白：`script.rs` 生成的 probe argv 为
`["{environment.python}", "script_host.py", "runner.py", "script_config.json", "--probe"]`，
依赖"相对路径按 package root 解析"；而 `environment.rs` 的 `run_probe` 把 probe 进程
cwd 设为 staging 内新建的空 `probe-runtime/`，`manifest.rs` 的 `resolve_argument` 又只
重写首个 argv（程序）的相对路径——没有任何一方把包文件放进该目录。七个 built-in
Runner 的 probe 全是自包含 `-c "import ..."`，从未暴露缺口；ensure 路径也没有脚本形态
manifest 的测试。create 阶段的 probe（`main.rs`，cwd=staging 包目录）能通过，把缺口
掩盖到第一次真实安装。

## Decision

probe 进程的工作目录改为 package root，与 entrypoint `working_directory = "package"`
语义一致；argv 相对路径按 package root 解析。需要临时目录的 probe 用既有
`{runtime.temp_root}` 模板显式声明（daemon 仍在 staging 内创建 `probe-runtime/` 供其
展开，提升后随目录保留）。该 cwd 与相对路径语义写入 `runner-manifest-v1.md` 的
Environment probe 节，成为 v1 契约的一部分。

## Alternatives considered

**把包文件复制进 probe-runtime。** `run_probe` 需要猜测引用了哪些文件或全量复制包
内容，副本随每个环境 fingerprint 重复落盘并与包内容漂移；否定。

**resolve_argument 重写全部相对参数到 package root。** 改变 argv 字面量的常规进程
语义（相对路径按 cwd 解析），probe 若想写临时文件会被重定向进包目录；只修 daemon
自己的生成器，第三方 manifest 作者写相对参数仍会踩坑；否定。

**生成器改用 `{package.root}/script_host.py` 绝对模板参数。** 能修好 Script Runner，
但 cwd 语义对任意 manifest 作者仍是未定义行为，同类缺陷换个写法即可复发；cwd 对齐
后此绕路无必要。

## Verification

- `tests/runner_environment_manager.rs::probe_resolves_relative_args_against_the_package_root`：
  probe argv 引用包内相对文件（Script Runner 形态），真实 uv 安装必须成功且 probe 经
  cwd 读到 `pyproject.toml`——钉住本回归。
- 既有 `probe_runs_under_the_synced_venv_interpreter` 改为向测试给定的绝对路径写
  sys.path 记录（原实现依赖旧 cwd 语义定位输出），核心断言不变：sys.path 含
  `installing/.venv/`，即 probe 运行在依赖已 sync 的 venv 解释器上。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过（2026-09-13）。
- 用户侧真实验证：重建 app 后对已落盘的 `org.hojoai.hojo-tts` 重新执行引擎安装并
  试听 TTS 输出。

## Consequences

- probe 与 entrypoint 的工作目录语义统一为 package root；manifest 作者可以把相对
  参数理解为"包内文件"，临时写入必须走 `{runtime.temp_root}`。
- 已按旧语义把输出写进 cwd 的第三方 probe（若有）会改为落在包目录内；当前仓库内
  七个 built-in 与 Script 生成器均不受影响。
- ensure 路径对脚本形态 manifest 有了直接测试覆盖，后续 probe 语义改动会先在此
  失败。
