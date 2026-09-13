# TTS 音色滚轮快速切换与即时试听

Status: implemented

Class: feature

Owner: this file

Related current decisions: [模型管理页子条目详细设置与 Runner 识别展示](2026-09-06-model-management-detail-settings.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)

## Problem

TTS 模型的音色数量决定了"逐个试听比对"是高频操作：Qwen3-TTS 有 9 个内置 speaker，Kokoro 类目录模型由 daemon 扫描 `voices/*.safetensors` 生成清单（`crates/ai-daemon/src/main.rs` 的 `list_model_voices`），可达数十个。改动前的交互带宽与该任务不匹配：

- 运行状态页「模型设置」sheet 的 menu 样式 Picker（NSPopUpButton）不响应滚轮，每比对一个音色都要"点开菜单→视觉定位→点击→等合成→等播放"，数十个音色的浏览成本线性放大；
- GUI 存在三份平行试听实现：`RunningModelSettingsView`（可选音色 + afplay）、`ModelDetailSheet` 工具栏（硬编码 `zf_001`，无音色选择）、`ModelsView` 管理页行内按钮（硬编码 `zf_001`）——其中两份连音色都换不了；
- 选中即持久化 default_voice 的语义没有区分"快速扫过"与"选定"，快速连续选择会对每个中间值写一次注册表；
- 所有试听都以 `Process` 跑 `/usr/bin/afplay` 并 `waitUntilExit()` 同步阻塞，且不保留进程引用——播放不可打断，在"逐个音色快速比对"场景下是结构性缺陷。

问题不依赖具体控件或手势实现：音色浏览需要一个低摩擦的连续选择输入通道，并把"选择行为"与"持久化/试听副作用"在时间上解耦。

## Decision

daemon 与 wire 契约零改动：`GET /api/models/{id}/voices` 与 `POST /api/models/{id}/voice` 原样复用，default_voice 的请求时应用语义（`runtime.rs:921`）不变。改动全部在 GUI 侧，收敛为一个共享控件 `VoiceSelectionControl`（`Views/VoiceSelectionControl.swift`）。

### 1. 滚轮步进：NSPopUpButton 子类消费 scrollWheel

`WheelSteppablePopUpButton` 继承 NSPopUpButton 并覆写 `scrollWheel(with:)`：事件被本控件消费（不带动外层页面滚动），按 `WheelStepAccumulator` 折算步进后 `selectItem(at:)` 并 `sendAction`——与菜单点击走同一 target/action 通路，由 Coordinator 驱动 binding 与 settle。点击仍弹出原生菜单直接跳转，键盘与 VoiceOver 行为继承自 NSPopUpButton。

步进规则沉淀为纯逻辑 `WheelStepAccumulator`：方向采用内容坐标语义（deltaY < 0 = 下一项）；系统已按用户的自然滚动偏好折算 delta，不叠加 `isDirectionInvertedFromDevice` 翻转；触控板精确滚动（`hasPreciseScrollingDeltas`）按阈值 10 累积、残余带入后续事件，鼠标滚轮离散 notch 每 tick 一步；`momentumPhase` 非空的惯性事件整体丢弃（含残余污染），防止松手后持续步进。部署目标 macOS 14，不使用 macOS 15+ 的 ScrollView phase API。

### 2. 选择与副作用解耦：settle 语义

滚轮每步只改本地 draft（纯 UI，无网络）。滚轮停止 450ms（settle）后合并为一次动作：与上次已持久化值不同时执行单次 `POST /voice`（`controller.api.setVoice` 直连，错误在控件内联展示），随后自动试听当前音色（控件内「滚轮试听」开关，默认开启）。菜单点击无滚轮会话，settle 立即触发，持久化行为与改动前的逐次持久化一致；滚轮窗口内关闭 sheet 由 `onDisappear` flush 兜底落库。

### 3. 试听：AVAudioPlayer 替换 afplay 子进程

`AVAudioPlayer(data:)` 直接播放内存中的合成音频：`stop()` 即打断，delegate 回调驱动「播放中…」状态，不再落盘临时文件、不再 `waitUntilExit()` 阻塞协作线程池。试听请求按代际计数：滚轮连续步进时，过期合成结果直接丢弃，新 settle 打断旧播放，任一时刻至多一路音频。合成仍走 daemon `POST /v1/audio/speech`。

### 4. 三处消费点收敛为单一 owner

- `RunningModelSettingsView`「语音」分区：原 Picker + 试听按钮替换为控件；
- `ModelDetailSheet`：TTS 目标新增「音色与试听」卡片（`voiceAuditionCard`，覆盖已注册模型；未注册/未加载由控件的状态分支呈现——voices endpoint 对未注册 ID 返回 ModelNotFound、合成要求模型驻留），删除工具栏硬编码 `zf_001` 的 `performTTSPreview`；
- `ModelsView` 管理页行内试听按钮移除：它只能放固定的 `zf_001`，严格弱于一步之遥的详情页卡片（行内本就有"点击查看详细设置"），属于死平行实现。

音色清单与当前 default voice 均由控件内从 daemon 读取，控件不持有模型状态。持久化直连 `api.setVoice` 而不经 `DaemonController.setVoice` 的 `refreshAfterSuccessfulMutation`：defaultVoice 没有其他 UI 消费点，控件每次打开都从 endpoint 重读，跳过刷新无观察差异。

## Alternatives considered

### NSEvent.addLocalMonitorForEvents 全局拦截

可保留现有 Picker、不改视图层级。但监听器过手整个 app 的所有滚动事件，需自行做 frame 命中测试与 sheet 生命周期守卫，吞/放事件的边界（多窗口、叠加 sheet）容易出错，且拦截规则无法脱离视图做纯逻辑单测。放弃。

### 依赖系统行为或升级部署目标

NSPopUpButton 无滚轮行为可依赖；SwiftUI 原生滚轮手势与 `onScrollPhaseChange` 需要 macOS 15+，与 `.macOS(.v14)` 部署目标冲突。放弃。

### 只加键盘/stepper，不做滚轮

不满足"滚轮快速切换"的需求本体。键盘方向键与 VoiceOver 行为由 NSPopUpButton 继承提供，作为无障碍补充而非滚轮替代。

### 试听仅手动

控件退化为"滚轮换名、按钮试听"，逐音色比对仍需两次输入；"扫过即听、停哪听哪"是该功能的核心价值。取自动试听 + 可关的折中。

### daemon 批量预合成音色样本

为消除单次试听的合成延迟引入新 wire 契约、缓存管理与 worker 占用；模型驻留后合成延迟为秒级，在 settle 防抖下可接受。过度设计，有意不做。

## Consequences

- **事件吞噬边界**：滚轮消费面就是 NSPopUpButton 自身的 hit area，与视觉一致；光标移开控件即恢复外层滚动。
- **自动试听的音频副作用**：滚轮停留即出声，对用户是新增的听觉打扰面；默认开启但可关，settle 防抖保证连续滚动不产生音频连发。
- **持久化时点后移**：滚轮路径从"选中即持久化"变为"settle 后持久化"，写库时点最多延迟 450ms；UI 立即反映选择，防抖窗口内关闭 sheet 由 flush 兜底。
- **管理页行内失去一键试听**：TTS 行的试听入口上移到详情页卡片（多一次点击），换取可选择音色、可打断与单一实现。
- **播放格式耦合**：daemon 返回 PCM WAV，AVAudioPlayer 原生支持；未来 Provider 契约若扩展非 WAV 格式，需同步复核播放通道。
- `zf_001` 仅存于 `VoiceSelectionControl.fallbackVoice`：与 daemon `POST /voice` 清空语义一致的 provider 内置缺省映射，非硬编码试听残留。
- 有意不做（Deferred）：音色元数据展示（性别/语言标签）、音色搜索与过滤、多音色顺序试听队列、daemon 侧音色样本预合成缓存、voices/voice endpoint 契约变更。

## Verification

- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 全套 68 例通过，其中 `VoiceWheelAccumulatorTests` 5 例固化累积器契约：离散 notch 每 tick 一步且向下滚为下一项、精确 delta 阈值累积与残余带入、惯性事件丢弃且不污染残余、零 delta 无副作用。
- 负向核查：GUI Sources 中 `afplay` 零残留；`zf_001` 仅剩 `fallbackVoice` 常量；`isPreviewing`/`performTTSPreview`/`previewVoice` 全部移除。
- 滚轮步进手感、外层滚动恢复、试听打断、触控板惯性不连跳等交互项属于交付后的人工界面验证边界，需真实 daemon 与驻留 TTS 模型逐项走查。

## 边界声明

本决策替换 [模型管理页子条目详细设置与 Runner 识别展示](2026-09-06-model-management-detail-settings.md) 中"TTS 试听"相关条款的行为（工具栏硬编码试听 → 详情页共享音色控件）；该记录的其余条款（识别展示、上下文设置、ID 管理等）继续由它拥有。
