import AVFoundation
import SwiftUI

/// 滚轮 delta → 步进数的纯累积器。
/// 方向为内容坐标语义：deltaY < 0（向下滚）= 列表下一项。系统已按用户的自然
/// 滚动偏好折算 delta，无需再叠加 isDirectionInvertedFromDevice 翻转。
/// - 触控手势（事件携带 phase：触控板与妙控鼠标表面）行为不变：精确滚动按
///   阈值累积，避免一次手势连跳多步；惯性（momentum）事件整体丢弃；
/// - 滚轮类设备（phase 为空）一次物理滚动是一串事件：高分辨率滚轮会把一个
///   notch 拆成多个精确 delta 事件，按事件步进会一次跳多行。突发窗口内固定
///   只步进一行，窗口内无后续事件（下一次独立滚动）才重新武装。
struct WheelStepAccumulator {
    /// 同一物理滚动的突发判定窗口：notch 内事件间隔通常 < 30ms，两次独立
    /// 滚动的间隔（含刻意连续滚动）通常 > 100ms。
    static let wheelBurstQuietInterval: TimeInterval = 0.1

    private var residual: CGFloat = 0
    private var wheelBurstArmed = true
    private var lastWheelEventTimestamp: TimeInterval = 0

    /// 返回该事件应产生的步进数：正值 = 下一项，负值 = 上一项。
    /// `gestureTimestamp` 为 `NSEvent.timestamp`（秒），驱动滚轮突发窗口判定。
    mutating func stepCount(
        forDeltaY deltaY: CGFloat,
        hasPreciseScrollingDeltas: Bool,
        isMomentum: Bool,
        hasGesturePhase: Bool,
        gestureTimestamp: TimeInterval
    ) -> Int {
        guard !isMomentum, deltaY != 0 else { return 0 }
        guard hasGesturePhase else {
            let elapsed = gestureTimestamp - lastWheelEventTimestamp
            lastWheelEventTimestamp = gestureTimestamp
            guard wheelBurstArmed || elapsed >= Self.wheelBurstQuietInterval else { return 0 }
            wheelBurstArmed = false
            return deltaY < 0 ? 1 : -1
        }
        guard hasPreciseScrollingDeltas else {
            return deltaY < 0 ? 1 : -1
        }
        let threshold: CGFloat = 10
        residual += deltaY
        var steps = 0
        while abs(residual) >= threshold {
            steps += residual < 0 ? 1 : -1
            residual += residual < 0 ? threshold : -threshold
        }
        return steps
    }
}

/// 滚轮可步进的音色下拉框：NSPopUpButton 子类消费滚轮事件逐项切换选中项；
/// 点击仍弹出原生菜单直接跳转，键盘与 VoiceOver 行为继承自 NSPopUpButton。
/// 事件只在光标位于控件内时被消费，控件外外层滚动视图行为不受影响。
private final class WheelSteppablePopUpButton: NSPopUpButton {
    var stepAccumulator = WheelStepAccumulator()

    override func scrollWheel(with event: NSEvent) {
        let steps = stepAccumulator.stepCount(
            forDeltaY: event.scrollingDeltaY,
            hasPreciseScrollingDeltas: event.hasPreciseScrollingDeltas,
            isMomentum: !event.momentumPhase.isEmpty,
            hasGesturePhase: !event.phase.isEmpty,
            gestureTimestamp: event.timestamp
        )
        for _ in 0..<abs(steps) {
            stepSelection(forward: steps > 0)
        }
    }

    private func stepSelection(forward: Bool) {
        let next = indexOfSelectedItem + (forward ? 1 : -1)
        guard next >= 0, next < numberOfItems else { return }
        selectItem(at: next)
        // 与菜单点击走同一 target/action 通路，由 Coordinator 驱动 binding 与 settle 逻辑。
        sendAction(action, to: target)
    }
}

private struct VoiceWheelPicker: NSViewRepresentable {
    let voices: [String]
    @Binding var selection: String?
    let onUserSelect: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> WheelSteppablePopUpButton {
        let button = WheelSteppablePopUpButton()
        button.pullsDown = false
        button.target = context.coordinator
        button.action = #selector(Coordinator.selectionChanged(_:))
        return button
    }

    func updateNSView(_ button: WheelSteppablePopUpButton, context: Context) {
        context.coordinator.parent = self
        context.coordinator.syncSelection(on: button)
    }

    @MainActor
    final class Coordinator: NSObject {
        var parent: VoiceWheelPicker
        private var syncedVoices: [String] = []

        init(_ parent: VoiceWheelPicker) {
            self.parent = parent
        }

        func syncSelection(on button: WheelSteppablePopUpButton) {
            if syncedVoices != parent.voices {
                syncedVoices = parent.voices
                button.removeAllItems()
                button.addItems(withTitles: parent.voices)
            }
            let index = parent.selection.flatMap { parent.voices.firstIndex(of: $0) } ?? 0
            if button.indexOfSelectedItem != index {
                button.selectItem(at: parent.voices.isEmpty ? -1 : index)
            }
        }

        @objc func selectionChanged(_ sender: NSPopUpButton) {
            guard let title = sender.titleOfSelectedItem else { return }
            parent.selection = title
            parent.onUserSelect(title)
        }
    }
}

/// 播放结束回调桥：AVAudioPlayer 的 delegate 需要长期存活的 NSObject 持有。
private final class PlaybackDelegate: NSObject, AVAudioPlayerDelegate {
    let onFinish: () -> Void

    init(onFinish: @escaping () -> Void) {
        self.onFinish = onFinish
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in onFinish() }
    }
}

/// TTS 音色选择 + 试听共享控件（运行状态页「模型设置」与模型详细设置共用）。
///
/// 交互契约（owner：docs/decisions/2026-09-13-tts-voice-wheel-selection.md）：
/// - 滚轮悬停在下拉框上逐项步进，只改本地 draft；触控板手势按阈值累积，
///   滚轮类设备一次物理滚动固定一行；
/// - 滚轮停止 settle（450ms）后合并为一次持久化 default voice + 一次自动试听
///   （可关）；菜单点击立即 settle；手动试听按钮随时可用；
/// - 新一次试听打断上一路播放（AVAudioPlayer），过期合成结果按代际丢弃；
/// - 音色清单与当前 default voice 均来自 daemon，本控件不持有模型状态。
struct VoiceSelectionControl: View {
    let modelID: String
    /// 模型未加载时试听不可用，但仍可切换并持久化默认音色。
    var allowsAudition: Bool = true
    var sampleText = "你好，我是 Mac AI 的语音合成，很高兴为你朗读这段文字。"

    @Environment(DaemonController.self) private var controller

    @State private var voices: [String] = []
    @State private var voicesLoaded = false
    @State private var loadFailed = false
    @State private var draftVoice: String?
    /// 最近一次已持久化到 daemon 的音色，用于 settle 去重与 onDisappear flush。
    @State private var persistedVoice: String?
    @State private var autoAudition = true
    @State private var isAuditioning = false
    @State private var errorMessage: String?
    @State private var settleTask: Task<Void, Never>?
    @State private var player: AVAudioPlayer?
    @State private var playbackDelegate: PlaybackDelegate?
    @State private var auditionGeneration = 0

    /// Provider 内置缺省音色，与 daemon `POST /voice` 清空语义一致。
    private static let fallbackVoice = "zf_001"
    private static let settleInterval = Duration.milliseconds(450)

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            statusOrPicker
            if let errorMessage {
                Text(errorMessage)
                    .font(.caption)
                    .foregroundStyle(Theme.danger)
            }
        }
        .task(id: modelID) { await loadVoices() }
        .onDisappear(perform: flushPendingPersist)
    }

    @ViewBuilder
    private var statusOrPicker: some View {
        if !voicesLoaded {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("正在读取音色…")
                    .foregroundStyle(.secondary)
            }
        } else if loadFailed {
            Text("无法读取音色清单（模型未注册或守护进程不可达）")
                .foregroundStyle(.secondary)
        } else if voices.isEmpty {
            Text("未找到可用音色")
                .foregroundStyle(.secondary)
        } else {
            pickerRows
        }
    }

    private var pickerRows: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Text("默认音色")
                    .font(.callout.weight(.medium))
                VoiceWheelPicker(voices: voices, selection: $draftVoice, onUserSelect: handleUserSelect)
                    .fixedSize(horizontal: true, vertical: true)
                    .frame(maxWidth: 260, alignment: .leading)
                    .help("滚轮上下快速切换音色；点击菜单可直接选择")
                if isAuditioning {
                    ProgressView().controlSize(.small)
                }
            }
            HStack(spacing: 12) {
                Button {
                    Task { await audition(draftVoice ?? "") }
                } label: {
                    Label(isAuditioning ? "播放中…" : "试听", systemImage: "speaker.wave.2.fill")
                }
                .labelStyle(.titleAndIcon)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(isAuditioning || !allowsAudition || controller.phase != .online)
                .help(allowsAudition ? "用当前音色合成一句示例文本并播放" : "模型加载后可试听")

                Toggle("滚轮试听", isOn: $autoAudition)
                    .toggleStyle(.checkbox)
                    .controlSize(.small)
                    .font(.caption)
                    .help("滚轮停止后自动试听当前音色")
                Spacer(minLength: 0)
            }
        }
    }

    // MARK: - 数据加载

    private func loadVoices() async {
        settleTask?.cancel()
        player?.stop()
        player = nil
        voicesLoaded = false
        loadFailed = false
        errorMessage = nil
        defer { voicesLoaded = true }
        guard let response = try? await controller.voices(for: modelID) else {
            loadFailed = true
            return
        }
        voices = response.voices
        let def = response.defaultVoice.flatMap { $0.isEmpty ? nil : $0 } ?? Self.fallbackVoice
        persistedVoice = def
        draftVoice = response.voices.contains(def) ? def : response.voices.first
    }

    // MARK: - 选择与 settle

    /// 滚轮步进与菜单点击的唯一入口：只改 draft，副作用延迟到 settle。
    private func handleUserSelect(_ voice: String) {
        errorMessage = nil
        draftVoice = voice
        settleTask?.cancel()
        settleTask = Task {
            try? await Task.sleep(for: Self.settleInterval)
            guard !Task.isCancelled else { return }
            await settle(on: voice)
        }
    }

    private func settle(on voice: String) async {
        if voice != persistedVoice {
            persistedVoice = voice
            do {
                try await controller.api.setVoice(modelID, voice: voice)
            } catch {
                errorMessage = "保存默认音色失败：\(DaemonController.message(for: error))"
            }
        }
        if autoAudition {
            await audition(voice)
        }
    }

    /// sheet 关闭时兜底：settle 窗口未到的最后一次选择仍要落库（尽力而为）。
    private func flushPendingPersist() {
        settleTask?.cancel()
        settleTask = nil
        player?.stop()
        player = nil
        guard let draftVoice, voicesLoaded, !loadFailed, draftVoice != persistedVoice else { return }
        persistedVoice = draftVoice
        Task { [modelID, draftVoice] in
            _ = try? await controller.api.setVoice(modelID, voice: draftVoice)
        }
    }

    // MARK: - 试听

    /// 合成示例句并用 AVAudioPlayer 播放；代际计数丢弃过期结果，新试听打断旧播放。
    private func audition(_ voice: String) async {
        guard !voice.isEmpty, allowsAudition, controller.phase == .online else { return }
        auditionGeneration += 1
        let generation = auditionGeneration
        player?.stop()
        player = nil
        isAuditioning = true
        do {
            let audio = try await controller.api.synthesizeSpeech(
                modelID: modelID,
                text: sampleText,
                voice: voice
            )
            guard generation == auditionGeneration else { return }
            let delegate = PlaybackDelegate(onFinish: {
                if generation == auditionGeneration {
                    isAuditioning = false
                }
            })
            playbackDelegate = delegate
            let newPlayer = try AVAudioPlayer(data: audio)
            newPlayer.delegate = delegate
            player = newPlayer
            newPlayer.play()
        } catch {
            guard generation == auditionGeneration else { return }
            isAuditioning = false
            errorMessage = "试听失败：\(DaemonController.message(for: error))"
        }
    }
}
