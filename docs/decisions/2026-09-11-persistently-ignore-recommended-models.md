# 管理页引擎与推荐模型条目的持久忽略和恢复

Status: implemented

Class: feature

Owner: this file

Related current decisions: [管理页置顶 Runner 引擎状态与安装控制、精简推荐模型条目](2026-09-07-management-view-engine-status-and-compact-recommendations.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)

## Problem

daemon 会向 GUI 提供完整的 Runner 和 Model Profile catalog。用户明确不需要其中某个条目时，管理页仍会在每次启动后持续展示它；持久忽略后若发生误操作，用户也需要明确的恢复路径。

## Decision

MacAIConsole 在引擎和推荐模型整行分别提供原生右键菜单动作“忽略引擎”和“忽略推荐”。两个行视图都使用矩形 `contentShape`，让包含透明空白在内的完整子条目区域均可触发对应操作。动作以 daemon 提供的稳定 Runner/Profile ID 为键，将两个独立忽略集合写入 `UserDefaults`；`ModelsView` 在两个既有列表入口排除对应 ID，并立即更新当前界面。

设置页提供“显示所有引擎”和“显示所有推荐模型”，分别清除对应忽略集合。恢复推荐模型时，既有的 `isDownloaded` 过滤继续生效，已下载模型不会重新进入推荐列表。

这些偏好仅属于 GUI 展示层。daemon 继续完整返回 catalog，也不感知客户端忽略状态；已下载模型仍依照既有模型仓库和注册表流程展示。

## Alternatives considered

- **由 daemon 保存忽略状态**：忽略是单个 GUI 用户的展示偏好，写入 Runtime Authority 会扩大 API 与持久化契约，并让其他客户端承担无关状态。
- **展示逐项忽略管理列表**：两个“显示所有”动作已经覆盖误操作恢复，不需要维护另一套条目列表和逐项状态。

## Consequences

- 被忽略的 Runner 或 Profile 子条目立即消失，并在应用后续启动时保持隐藏；推荐模型全部被过滤后，其区块依照既有逻辑一并隐藏。
- Runner/Profile ID 变化会被视为新条目并重新显示，这是 catalog 项目身份变化的自然结果。
- 恢复动作按类别清空全部忽略项，不提供逐项恢复。

## Verification

- `AppSettingsTests.testIgnoredRecommendationsPersistWithoutDuplicates` 验证 Profile ID 集合从空状态写入、去重并可从同一 `UserDefaults` suite 重新读取。
- `AppSettingsTests.testIgnoredItemsCanBeRestoredByCategory` 验证两个类别独立持久化并可分别清除，互不影响。
- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 验证 MacAIConsole 测试套件与界面组合可编译。
- 运行 MacAIConsole，验证引擎子条目整行均可打开“忽略引擎”菜单，并分别忽略一个引擎和推荐模型；从设置页恢复后返回管理页，验证引擎重新显示、未下载的推荐模型重新显示且已下载模型不显示。此项属于交付后的人工界面验证边界。
