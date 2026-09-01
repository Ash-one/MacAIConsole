import AppKit
import SwiftUI

struct LogsView: View {
    @Environment(DaemonController.self) private var controller
    @AppStorage(AppSettings.logLevelKey) private var logLevelRaw = LogLevel.info.rawValue
    @State private var source: LogSource = .daemon
    @State private var snapshot = RecentLogSnapshot(entries: [], modifiedAt: nil)
    @State private var loadError: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            controls
            metadata
            logContent
        }
        .padding(Theme.Space.page)
        .navigationTitle("最近日志")
        .task(id: source) {
            while !Task.isCancelled {
                await reload()
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
        .onChange(of: logLevelRaw) { _, newValue in
            guard let level = LogLevel(rawValue: newValue) else { return }
            Task {
                do {
                    try await controller.setLogLevel(level)
                    loadError = nil
                } catch {
                    loadError = "切换日志级别失败：\(DaemonController.message(for: error))"
                }
            }
        }
    }

    private var controls: some View {
        HStack(spacing: 10) {
            Picker("日志来源", selection: $source) {
                ForEach(LogSource.allCases) { source in
                    Text(source.title).tag(source)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 220)

            Picker("日志级别", selection: $logLevelRaw) {
                Text("Info").tag(LogLevel.info.rawValue)
                Text("Debug").tag(LogLevel.debug.rawValue)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .accessibilityLabel("日志级别")
            .frame(width: 150)
            .help("Info 显示常规、警告和错误日志；Debug 显示全部日志")

            Spacer(minLength: 12)

            Button {
                revealLog()
            } label: {
                Label("在访达中显示", systemImage: "folder")
            }

            Button {
                Task { await reload() }
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
        }
    }

    private var metadata: some View {
        HStack(spacing: 8) {
            Text(source.fileURL.path)
                .font(.caption2.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(Theme.inset)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline)
                )
                .textSelection(.enabled)
                .help(source.fileURL.path)
            Spacer(minLength: 12)
            Text(summaryText)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .fixedSize()
        }
    }

    @ViewBuilder
    private var logContent: some View {
        if let loadError {
            ErrorBanner(text: loadError, onClose: { self.loadError = nil })
        }

        SectionCard(title: source == .gui ? "MacAIConsole GUI" : "aiworkd daemon", icon: "terminal") {
            if visibleEntries.isEmpty {
                ContentUnavailableView(
                    "暂无日志",
                    systemImage: "doc.text.magnifyingglass",
                    description: Text(emptyDescription)
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                LogConsoleView(entries: visibleEntries)
                    .id("\(source.rawValue)-\(logLevelRaw)")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Theme.inset)
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hairline))
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var summaryText: String {
        guard let modifiedAt = snapshot.modifiedAt else { return "\(selectedLogLevel.title) · 0 行" }
        return "\(selectedLogLevel.title) · \(visibleEntries.count)/\(snapshot.lineCount) 行 · \(modifiedAt.formatted(date: .omitted, time: .standard))"
    }

    private var selectedLogLevel: LogLevel {
        LogLevel(rawValue: logLevelRaw) ?? .info
    }

    private var visibleEntries: [LogEntry] {
        snapshot.entries.filter(selectedLogLevel.includes)
    }

    private var emptyDescription: String {
        if !snapshot.entries.isEmpty {
            return "当前 \(selectedLogLevel.title) 级别下暂无可显示日志。"
        }
        switch source {
        case .gui: return "GUI 启动和操作记录会显示在这里。"
        case .daemon: return "启动 aiworkd 后，它的标准输出和错误日志会显示在这里。"
        }
    }

    /// 后台线程读日志；来源切换会取消旧 task，读完后检查取消避免旧结果覆盖新来源。
    private func reload() async {
        do {
            let newSnapshot = try await RecentLogReader.readInBackground(source: source)
            guard !Task.isCancelled else { return }
            snapshot = newSnapshot
            loadError = nil
        } catch {
            guard !Task.isCancelled else { return }
            loadError = "读取日志失败：\(error.localizedDescription)"
        }
    }

    private func revealLog() {
        let fileManager = FileManager.default
        let url = source.fileURL
        if fileManager.fileExists(atPath: url.path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            try? fileManager.createDirectory(at: LogFiles.directory, withIntermediateDirectories: true)
            NSWorkspace.shared.open(LogFiles.directory)
        }
        AppLogger.info("已在访达中显示 \(source.title) 日志")
    }
}
