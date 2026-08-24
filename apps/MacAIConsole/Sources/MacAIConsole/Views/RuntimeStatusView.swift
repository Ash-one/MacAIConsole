import SwiftUI

struct RuntimeStatusView: View {
    @Environment(DaemonController.self) private var controller

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let error = controller.lastError {
                    ErrorBanner(text: error)
                }
                headerCard
                statsGrid
                loadedModelsSection
                providerSection
            }
            .padding(20)
        }
        .navigationTitle("运行状态")
    }

    private var headerCard: some View {
        GroupBox {
            HStack(spacing: 16) {
                StatusBadge(phase: controller.phase)
                Spacer(minLength: 0)
                switch controller.phase {
                case .offline:
                    Button("启动 aiworkd") { controller.startDaemon() }
                        .buttonStyle(.borderedProminent)
                case .online:
                    Button("停止", role: .destructive) { controller.stopDaemon() }
                        .buttonStyle(.bordered)
                case .starting, .stopping:
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(controller.phase == .starting ? "正在启动…" : "正在停止…")
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .padding(4)
        }
    }

    private var statsGrid: some View {
        let info = controller.info
        return LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: 12)], spacing: 12) {
            StatCard(title: "版本", value: info?.version ?? "—")
            StatCard(title: "PID", value: info.map { "\($0.pid)" } ?? "—")
            StatCard(title: "运行时长", value: info.map { Format.uptime($0.uptimeSecs) } ?? "—")
            StatCard(title: "活跃请求", value: info.map { "\($0.activeRequests)" } ?? "—")
        }
    }

    private var loadedModelsSection: some View {
        GroupBox("加载中的模型") {
            if let models = controller.info?.loadedModels, !models.isEmpty {
                VStack(spacing: 0) {
                    ForEach(models) { model in
                        ModelRow(model: model)
                        if model.id != models.last?.id { Divider() }
                    }
                }
                .padding(.horizontal, 4)
            } else {
                Text(controller.phase == .online ? "当前没有加载中的模型" : "守护进程离线，暂无数据")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
            }
        }
    }

    private var providerSection: some View {
        GroupBox("Provider 状态") {
            if controller.providers.isEmpty {
                Text("暂无 Provider 数据")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
            } else {
                VStack(spacing: 0) {
                    ForEach(controller.providers) { provider in
                        ProviderRow(entry: provider)
                        if provider.id != controller.providers.last?.id { Divider() }
                    }
                }
                .padding(.horizontal, 4)
            }
        }
    }
}

struct ModelRow: View {
    @Environment(DaemonController.self) private var controller
    let model: LoadedModel

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.id)
                    .font(.body.weight(.medium))
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            if controller.busyModelIDs.contains(model.id) {
                ProgressView().controlSize(.small)
            } else {
                Button("卸载") {
                    Task { await controller.unload(model.id) }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(controller.phase != .online)
            }
        }
        .padding(.vertical, 6)
    }

    private var subtitle: String {
        var parts = [model.provider]
        if model.state != "ready" { parts.append("状态：\(model.state)") }
        if let memory = model.memoryEstimate, memory > 0 { parts.append(Format.bytes(memory)) }
        if let keepAlive = model.keepAlive, !keepAlive.isEmpty { parts.append("keep_alive：\(keepAlive)") }
        parts.append("\(Format.relativeTime(model.loadedAt))加载")
        return parts.joined(separator: " · ")
    }
}

struct ProviderRow: View {
    let entry: ProviderEntry

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(entry.descriptor.id)
                        .font(.body.weight(.medium))
                    if let isolation = entry.descriptor.isolation, !isolation.isEmpty {
                        Text(isolation)
                            .font(.caption2)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(.secondary.opacity(0.12)))
                            .foregroundStyle(.secondary)
                    }
                }
                if let capabilities = entry.descriptor.capabilities, !capabilities.isEmpty {
                    Text(capabilities.joined(separator: "、"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let reason = entry.status.reason, !reason.isEmpty {
                    Text(reason)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
            HStack(spacing: 4) {
                Circle()
                    .fill(entry.status.available ? Color.green : Color.red)
                    .frame(width: 8, height: 8)
                Text(statusText)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
            }
            .fixedSize()
        }
        .padding(.vertical, 6)
    }

    private var statusText: String {
        var text = entry.status.ready ? "就绪" : (entry.status.available ? "可用" : "不可用")
        if let device = entry.status.effectiveDevice, !device.isEmpty {
            text += " · \(device)"
        }
        return text
    }
}