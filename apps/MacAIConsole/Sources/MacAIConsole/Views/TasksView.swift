import AppKit
import SwiftUI

struct TasksView: View {
    @Environment(DaemonController.self) private var controller
    @State private var selectedTask: InferenceTaskSummary?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.sectionSpacing) {
                if controller.phase != .online {
                    OfflineHint { controller.startDaemon() }
                }
                if let error = controller.tasksError {
                    ErrorBanner(text: error)
                }
                runningSection
                completedSection
            }
            .padding(Theme.Space.page)
        }
        .navigationTitle("任务记录")
        .sheet(item: $selectedTask) { task in
            TaskDetailSheet(task: task)
                .environment(controller)
        }
        .task {
            controller.bootstrapIfNeeded()
        }
    }

    private var runningSection: some View {
        SectionCard(title: "正在运行", icon: "bolt", accessory: {
            Chip(text: "\(controller.runningTasks.count)")
        }) {
            if controller.runningTasks.isEmpty {
                EmptyHint(
                    text: controller.phase == .online ? "当前没有正在运行的任务" : "守护进程离线，暂无数据",
                    systemImage: "bolt.slash"
                )
            } else {
                VStack(spacing: 0) {
                    ForEach(controller.runningTasks) { task in
                        TaskRow(task: task) { selectedTask = task }
                        if task.id != controller.runningTasks.last?.id { HairlineDivider() }
                    }
                }
            }
        }
    }

    private var completedSection: some View {
        SectionCard(title: "最近完成", icon: "checkmark.circle", accessory: {
            Chip(text: "\(controller.completedTasks.count)")
        }) {
            if controller.completedTasks.isEmpty {
                EmptyHint(
                    text: controller.phase == .online ? "当前 aiworkd 会话中还没有已完成任务" : "守护进程离线，暂无数据",
                    systemImage: "checkmark.circle"
                )
            } else {
                VStack(spacing: 0) {
                    ForEach(controller.completedTasks) { task in
                        TaskRow(task: task) { selectedTask = task }
                        if task.id != controller.completedTasks.last?.id { HairlineDivider() }
                    }
                }
            }
        }
    }
}

private struct TaskRow: View {
    let task: InferenceTaskSummary
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: task.iconType)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 7) {
                    Text(task.kindTitle)
                        .font(.body.weight(.semibold))
                    Text(task.inputPreview.isEmpty ? "—" : task.inputPreview)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Text(modelLine)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let sttMetricsLine {
                    Text(sttMetricsLine)
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 12)
            if task.isRunning {
                runningPill
            } else {
                Chip(text: task.statusTitle, color: statusColor)
            }
            VStack(alignment: .trailing, spacing: 2) {
                Text(durationText)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .fixedSize()
                if !task.isRunning {
                    Text(Format.relativeTime(milliseconds: task.completedAtMs))
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .fixedSize()
                }
            }
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 8)
        .hoverableRow()
        .onTapGesture(perform: onSelect)
        .help("点击查看任务详情")
    }

    /// 运行中状态药丸：脉冲圆点 + amber 文字。
    private var runningPill: some View {
        HStack(spacing: 5) {
            StatusDot(color: Theme.warning, size: 6, pulse: true)
            Text("运行中")
                .font(.caption2.weight(.semibold))
        }
        .foregroundStyle(Theme.warning)
        .padding(.horizontal, 7)
        .padding(.vertical, 2.5)
        .background(Capsule().fill(Theme.warning.opacity(0.13)))
        .overlay(Capsule().strokeBorder(Theme.warning.opacity(0.22)))
    }

    private var modelLine: String {
        var parts = [task.model]
        if let provider = task.provider, !provider.isEmpty {
            parts.append(provider)
        }
        return parts.joined(separator: " · ")
    }

    private var sttMetricsLine: String? {
        guard task.kind == "stt" else { return nil }
        let rtf = task.isRunning ? "计算中" : Format.realTimeFactor(task.realTimeFactor)
        return "输入音频 \(Format.duration(milliseconds: task.audioDurationMs)) · RTF \(rtf)"
    }

    private var durationText: String {
        task.isRunning
            ? Format.runningDuration(startedAtMs: task.startedAtMs)
            : Format.duration(milliseconds: task.durationMs)
    }

    private var statusColor: Color {
        switch task.status {
        case "succeeded": Theme.success
        case "failed": Theme.danger
        case "cancelled": Theme.warning
        default: .secondary
        }
    }
}

private struct TaskDetailSheet: View {
    @Environment(DaemonController.self) private var controller
    @Environment(\.dismiss) private var dismiss

    let task: InferenceTaskSummary
    @State private var detail: InferenceTaskDetail?
    @State private var loadError: String?
    @State private var detailFootnote: String?

    private var displaySummary: InferenceTaskSummary {
        detail?.summary ?? task
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                if let loadError {
                    ErrorBanner(text: loadError)
                    if let detailFootnote {
                        Text(detailFootnote)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                basicInfo
                inputSection
                outputSection
                if let error = detail?.error ?? displaySummary.error {
                    VStack(alignment: .leading, spacing: 10) {
                        Label("错误信息", systemImage: "exclamationmark.triangle.fill")
                            .font(.callout.weight(.semibold))
                            .foregroundStyle(Theme.danger)
                        Text(error)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                    }
                    .padding(14)
                    .background(
                        RoundedRectangle(cornerRadius: Theme.Radius.card, style: .continuous)
                            .fill(Theme.danger.opacity(0.08))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.Radius.card, style: .continuous)
                            .strokeBorder(Theme.danger.opacity(0.30))
                    )
                }
                if detail == nil, loadError == nil {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("正在读取任务详情…")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .padding(Theme.Space.page)
        }
        .frame(minWidth: 600, minHeight: 560)
        .navigationTitle("任务详情")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("关闭") { dismiss() }
            }
        }
        .task(id: task.id) {
            await loadDetails()
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            ModelTypeIcon(type: displaySummary.iconType)
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 8) {
                    Text(displaySummary.kindTitle)
                        .font(.title2.weight(.semibold))
                    Chip(text: displaySummary.statusTitle, color: statusColor)
                }
                HStack(spacing: 7) {
                    Text(displaySummary.id)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    Button {
                        let pasteboard = NSPasteboard.general
                        pasteboard.clearContents()
                        pasteboard.setString(displaySummary.id, forType: .string)
                    } label: {
                        Image(systemName: "doc.on.doc")
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help("复制任务 ID")
                }
            }
            Spacer(minLength: 0)
            if displaySummary.isRunning { ProgressView().controlSize(.small) }
        }
    }

    private var basicInfo: some View {
        SectionCard(title: "基本信息", icon: "info.circle") {
            VStack(alignment: .leading, spacing: 7) {
                LabeledContent("模型", value: displaySummary.model)
                LabeledContent("Provider", value: displaySummary.provider ?? "—")
                LabeledContent("开始时间", value: Format.absoluteTime(milliseconds: displaySummary.startedAtMs))
                LabeledContent("结束时间", value: displaySummary.isRunning ? "仍在运行" : Format.absoluteTime(milliseconds: displaySummary.completedAtMs))
                LabeledContent("耗时", value: durationText)
                if displaySummary.kind == "stt" {
                    LabeledContent("输入音频时长", value: Format.duration(milliseconds: displaySummary.audioDurationMs))
                    LabeledContent(
                        "RTF",
                        value: displaySummary.isRunning
                            ? "计算中"
                            : Format.realTimeFactor(displaySummary.realTimeFactor)
                    )
                }
            }
        }
    }

    @ViewBuilder
    private var inputSection: some View {
        SectionCard(title: "输入内容", icon: "arrow.down.to.line") {
            if let request = detail?.request {
                switch displaySummary.kind {
                case "chat": chatInput(request)
                case "stt": sttInput(request)
                case "tts": ttsInput(request)
                default: genericInput(request)
                }
            } else {
                Text("详情暂不可用")
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func chatInput(_ request: InferenceTaskRequest) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if let messages = request.messages, !messages.isEmpty {
                ForEach(Array(messages.enumerated()), id: \.offset) { _, message in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(message.role)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        textBlock(message.content)
                    }
                }
            } else {
                Text("没有消息")
                    .foregroundStyle(.secondary)
            }
            Divider()
            metadataLine("流式", value: request.stream == true ? "是" : "否")
            metadataLine("temperature", value: request.temperature.map { String(format: "%.2f", $0) } ?? "—")
            metadataLine("max_tokens", value: request.maxTokens.map(String.init) ?? "—")
            truncationNotice(isTruncated: detail?.requestTruncated == true)
        }
    }

    @ViewBuilder
    private func sttInput(_ request: InferenceTaskRequest) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            metadataLine("文件名", value: request.fileName ?? "—")
            metadataLine("文件大小", value: Format.bytes(request.fileSizeBytes))
            metadataLine("语言", value: request.language ?? "—")
            metadataLine("格式", value: request.format ?? "—")
            truncationNotice(isTruncated: detail?.requestTruncated == true)
        }
    }

    @ViewBuilder
    private func ttsInput(_ request: InferenceTaskRequest) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            textBlock(request.inputText ?? "—")
            metadataLine("音色", value: request.voice ?? "—")
            metadataLine("格式", value: request.format ?? "—")
            metadataLine("语速", value: request.speed.map { String(format: "%.2f", $0) } ?? "—")
            truncationNotice(isTruncated: detail?.requestTruncated == true)
        }
    }

    @ViewBuilder
    private func genericInput(_ request: InferenceTaskRequest) -> some View {
        textBlock(request.inputText ?? request.fileName ?? "—")
        truncationNotice(isTruncated: detail?.requestTruncated == true)
    }

    @ViewBuilder
    private var outputSection: some View {
        SectionCard(title: "输出内容", icon: "arrow.up.forward") {
            if let result = detail?.result {
                VStack(alignment: .leading, spacing: 8) {
                    if let outputText = result.outputText, !outputText.isEmpty {
                        textBlock(outputText)
                    } else if displaySummary.isRunning {
                        Text("正在生成…")
                            .foregroundStyle(.secondary)
                    }
                    switch displaySummary.kind {
                    case "tts":
                        metadataLine("Content-Type", value: result.contentType ?? "—")
                        metadataLine("生成字节数", value: Format.bytes(result.byteCount))
                    case "stt":
                        metadataLine("识别语言", value: result.language ?? "—")
                    case "chat":
                        if let finishReason = result.finishReason {
                            metadataLine("finish_reason", value: finishReason)
                        }
                        if let prompt = result.promptTokens {
                            metadataLine("prompt tokens", value: "\(prompt)")
                        }
                        if let completion = result.completionTokens {
                            metadataLine("completion tokens", value: "\(completion)")
                        }
                        if let total = result.totalTokens {
                            metadataLine("total tokens", value: "\(total)")
                        }
                    default:
                        if let finishReason = result.finishReason {
                            metadataLine("finish_reason", value: finishReason)
                        }
                        if let language = result.language {
                            metadataLine("语言", value: language)
                        }
                    }
                    truncationNotice(isTruncated: detail?.resultTruncated == true)
                }
            } else {
                Text(displaySummary.isRunning ? "正在生成…" : "没有结果")
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func textBlock(_ text: String) -> some View {
        ScrollView(.vertical) {
            Text(text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
                .padding(10)
        }
        .frame(maxHeight: 190)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .fill(Theme.inset)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .strokeBorder(Theme.hairline)
        )
    }

    @ViewBuilder
    private func metadataLine(_ title: String, value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .foregroundStyle(.secondary)
            Spacer(minLength: 12)
            Text(value)
                .multilineTextAlignment(.trailing)
                .textSelection(.enabled)
        }
        .font(.callout)
    }

    @ViewBuilder
    private func truncationNotice(isTruncated: Bool) -> some View {
        if isTruncated {
            Text("内容过长，仅保留前 64 KiB")
                .font(.caption)
                .foregroundStyle(Theme.warning)
        }
    }

    private var durationText: String {
        displaySummary.isRunning
            ? Format.runningDuration(startedAtMs: displaySummary.startedAtMs)
            : Format.duration(milliseconds: displaySummary.durationMs)
    }

    private var statusColor: Color {
        switch displaySummary.status {
        case "succeeded": Theme.success
        case "failed": Theme.danger
        case "cancelled": Theme.warning
        case "running": Theme.warning
        default: .secondary
        }
    }

    private func loadDetails() async {
        while !Task.isCancelled {
            do {
                let latest = try await controller.task(id: task.id)
                detail = latest
                loadError = nil
                detailFootnote = nil
                guard latest.isRunning else { return }
                try await Task.sleep(nanoseconds: 1_000_000_000)
            } catch is CancellationError {
                return
            } catch {
                if case let DaemonError.http(status, _) = error, status == 404 {
                    // 任务不在 daemon 历史里：已完成任务对应容量淘汰；
                    // 仍显示运行中的任务说明守护进程重启、历史被清除——两者都是终态，停止轮询。
                    if displaySummary.isRunning {
                        loadError = "任务记录已不存在：守护进程可能已重启，无法继续追踪运行状态。"
                        detailFootnote = "仍显示列表中的摘要。"
                    } else {
                        loadError = "任务可能已被历史容量淘汰；仍显示列表中的摘要。"
                        detailFootnote = nil
                    }
                    return
                }
                // 瞬时失败（网络抖动、守护进程重启中、超时等）：任务仍预期在运行时继续轮询，
                // 恢复后自动清除错误；任务已结束时如实展示失败原因，不再声称容量淘汰。
                guard displaySummary.isRunning else {
                    loadError = "读取任务详情失败：\(DaemonController.message(for: error))"
                    detailFootnote = nil
                    return
                }
                loadError = "读取任务详情暂时失败，正在重试…"
                detailFootnote = nil
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }
}
