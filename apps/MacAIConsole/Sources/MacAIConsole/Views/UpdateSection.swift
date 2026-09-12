import SwiftUI

/// 设置页「关于」内的应用更新区块：版本展示、检查与安装。
struct UpdateSection: View {
    @Environment(AppUpdateState.self) private var updateState
    @AppStorage(AppSettings.autoUpdateCheckKey) private var autoCheck = true

    var body: some View {
        LabeledContent {
            HStack(spacing: 10) {
                statusView
                if case .available = updateState.status {
                    Button("立即更新") {
                        Task { await updateState.downloadAndInstall() }
                    }
                } else {
                    Button("检查更新") {
                        Task { await updateState.checkForUpdates() }
                    }
                    .disabled(isBusy)
                }
            }
        } label: {
            VStack(alignment: .leading) {
                HStack(spacing: 6) {
                    Text("当前版本")
                    InfoTip(text: "应用内更新会从 GitHub Releases 下载 DMG，校验 SHA-256 后替换当前应用并自动重启。")
                }
                Text(AppUpdater.currentVersion ?? "开发构建")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }

        Toggle("启动时自动检查更新", isOn: $autoCheck)
    }

    @ViewBuilder
    private var statusView: some View {
        switch updateState.status {
        case .idle:
            EmptyView()
        case .checking, .downloading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(updateState.status == .checking ? "正在检查…" : "正在下载…")
                    .foregroundStyle(.secondary)
            }
        case .installing:
            Text("正在安装，应用即将重启…")
                .foregroundStyle(.secondary)
        case .upToDate:
            Label("已是最新版本", systemImage: "checkmark.circle.fill")
                .foregroundStyle(Theme.success)
        case .available(let release):
            Label("发现新版本 \(release.tagName)", systemImage: "arrow.down.circle.fill")
                .foregroundStyle(Theme.accent)
        case .failed(let message):
            Label(message, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(Theme.danger)
        }
    }

    private var isBusy: Bool {
        switch updateState.status {
        case .checking, .downloading, .installing: true
        default: false
        }
    }
}
