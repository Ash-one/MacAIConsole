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
        .padding(24)
        .navigationTitle("最近日志")
        .task(id: source) {
            while !Task.isCancelled {
                reload()
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
            .frame(width: 150)
            .help("Info 显示常规、警告和错误日志；Debug 显示全部日志")

            Spacer(minLength: 12)

            Button {
                revealLog()
            } label: {
                Label("在访达中显示", systemImage: "folder")
            }

            Button {
                reload()
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
        }
    }

    private var metadata: some View {
        HStack(spacing: 8) {
            Text(source.fileURL.path)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
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

        GroupBox {
            if visibleEntries.isEmpty {
                ContentUnavailableView(
                    "暂无日志",
                    systemImage: "doc.text.magnifyingglass",
                    description: Text(emptyDescription)
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                GeometryReader { geometry in
                    ScrollViewReader { proxy in
                        ScrollView([.horizontal, .vertical]) {
                            LazyVStack(alignment: .leading, spacing: 2) {
                                ForEach(visibleEntries) { entry in
                                    Text(entry.text)
                                        .font(.system(size: 11, design: .monospaced))
                                        .foregroundStyle(color(for: entry.level))
                                        .multilineTextAlignment(.leading)
                                        .textSelection(.enabled)
                                        .fixedSize(horizontal: true, vertical: true)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }
                                Color.clear
                                    .frame(height: 1)
                                    .id("log-bottom")
                            }
                            .frame(
                                minWidth: max(geometry.size.width - 20, 0),
                                minHeight: max(geometry.size.height - 20, 0),
                                alignment: .topLeading
                            )
                            .padding(10)
                        }
                        .background(Color(nsColor: .textBackgroundColor).opacity(0.55))
                        .clipShape(RoundedRectangle(cornerRadius: 7))
                        .onAppear {
                            proxy.scrollTo("log-bottom", anchor: .bottom)
                        }
                        .onChange(of: snapshot.entries) { _, _ in
                            proxy.scrollTo("log-bottom", anchor: .bottom)
                        }
                        .onChange(of: logLevelRaw) { _, _ in
                            proxy.scrollTo("log-bottom", anchor: .bottom)
                        }
                    }
                }
            }
        } label: {
            Text(source == .gui ? "MacAIConsole GUI" : "aiworkd daemon")
                .font(.title3.weight(.semibold))
                .padding(.bottom, 8)
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

    private func reload() {
        do {
            snapshot = try RecentLogReader.read(source: source)
            loadError = nil
        } catch {
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

    private func color(for level: LogLevel) -> Color {
        switch level {
        case .debug: .secondary
        case .info: .primary
        case .warning: .orange
        case .error: .red
        }
    }
}
