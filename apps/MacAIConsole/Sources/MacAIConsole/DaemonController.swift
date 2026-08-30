import Darwin
import Foundation
import Observation
import SystemConfiguration

/// 守护进程生命周期 + 数据轮询的状态机。
///
/// 状态：offline → starting → online；online → stopping → offline。
/// 轮询循环按状态决定节奏：启动/停止中 0.4s、在线 2s、离线 1s。
/// 离线时仍持续探活，用户在终端手动拉起的 aiworkd 会被自动收编为在线。
@MainActor
@Observable
final class DaemonController {
    enum Phase: Equatable {
        case offline
        case starting
        case online
        case stopping
    }

    private(set) var phase: Phase = .offline
    var info: RuntimeInfo?
    var providers: [ProviderEntry] = []
    var registeredModels: [ModelEntry] = []
    private(set) var runningTasks: [InferenceTaskSummary] = []
    private(set) var completedTasks: [InferenceTaskSummary] = []
    private(set) var tasksError: String?

    /// 与 daemon 端 MAX_COMPLETED_TASKS 对齐的完成历史上限；刷新按此值拉取。
    /// 历史到上限后旧任务被淘汰，累计数无法区分「恰好」与「更多」，文案按「N+」呈现。
    static let completedHistoryLimit = 100

    /// 累计任务统计（当前 daemon 会话）：运行中 + 最近完成。
    var cumulativeTaskCountText: String {
        let total = runningTasks.count + completedTasks.count
        return total >= Self.completedHistoryLimit ? "\(total)+" : "\(total)"
    }

    var lastError: String? {
        didSet {
            guard let lastError, lastError != oldValue else { return }
            logError(lastError)
        }
    }
    var busyModelIDs: Set<String> = []
    var busyRecommendationIDs: Set<String> = []

    let api: DaemonAPI

    private var loopTask: Task<Void, Never>?
    private var childProcess: Process?
    private var logHandle: FileHandle?
    private var stopDeadline: Date?
    private var restartAfterStop = false
    private var refreshIsDegraded = false
    private let logsEnabled: Bool

    init(api: DaemonAPI = DaemonAPI(), logsEnabled: Bool = true) {
        self.api = api
        self.logsEnabled = logsEnabled
        logInfo("MacAIConsole GUI 已启动")
    }

    // MARK: - 生命周期

    func bootstrapIfNeeded() {
        guard loopTask == nil else { return }
        logInfo("守护进程状态轮询已启动")
        loopTask = Task { [weak self] in
            while !Task.isCancelled {
                let interval = await self?.tick() ?? 1.0
                try? await Task.sleep(for: .seconds(interval))
            }
        }
        guard AppSettings.autoStartDaemon else {
            logInfo("已关闭 aiworkd 自动启动")
            return
        }
        Task { [weak self] in
            guard let self, !(await self.api.isHealthy()) else { return }
            self.startDaemon()
        }
    }

    func startDaemon() {
        guard phase == .offline else { return }
        guard let binary = Self.resolveBinary() else {
            lastError = "未找到 aiworkd 可执行文件。请先在 MacAI 仓库构建（cargo build --release -p ai-daemon），或设置 AIWORKD_PATH 环境变量。"
            return
        }
        lastError = nil
        phase = .starting
        logInfo("正在启动 aiworkd：\(binary.path)")
        do {
            let (process, log) = try Self.spawn(binary: binary)
            childProcess = process
            logHandle = log
            logInfo("aiworkd 进程已拉起，PID \(process.processIdentifier)")
            process.terminationHandler = { [weak self] proc in
                Task { @MainActor in
                    guard let self, self.childProcess === proc else { return }
                    if self.phase == .stopping {
                        self.logInfo("aiworkd 已按请求退出，code \(proc.terminationStatus)")
                        return
                    }
                    self.lastError = "aiworkd 意外退出（code \(proc.terminationStatus)）。日志：应用支持目录/MacAIConsole/logs/aiworkd.log"
                    self.childProcess = nil
                }
            }
        } catch {
            phase = .offline
            lastError = "启动失败：\(error.localizedDescription)"
        }
    }

    func stopDaemon() {
        guard phase == .online else { return }
        phase = .stopping
        stopDeadline = Date.now.addingTimeInterval(8)
        if let pid = info?.pid, pid > 0 {
            logInfo("正在停止 aiworkd，PID \(pid)")
            kill(pid_t(pid), SIGTERM)
        } else if let child = childProcess, child.isRunning {
            logInfo("正在停止 GUI 拉起的 aiworkd")
            child.terminate()
        }
    }

    func restartDaemon() {
        logInfo("已请求重启 aiworkd")
        restartAfterStop = true
        switch phase {
        case .offline:
            restartAfterStop = false
            startDaemon()
        case .online:
            stopDaemon()
        case .starting, .stopping:
            break
        }
    }

    // MARK: - 数据操作

    func refresh() async throws {
        async let infoRequest = api.runtimeInfo()
        async let modelsRequest = api.models()
        async let providersRequest = api.providers()
        async let tasksRequest = api.tasks(completedLimit: Self.completedHistoryLimit)

        let info = try await infoRequest
        let models = try? await modelsRequest
        let providers = try? await providersRequest
        let tasks = try? await tasksRequest
        self.info = info
        if let models { registeredModels = models }
        if let providers { self.providers = providers }
        if let tasks {
            runningTasks = tasks.running
            completedTasks = tasks.completed
            tasksError = nil
        } else {
            let message = "任务记录暂时无法读取：守护进程未返回有效数据"
            if tasksError != message { logWarning(message) }
            tasksError = message
        }
    }

    func task(id: String) async throws -> InferenceTaskDetail {
        try await api.task(id: id)
    }

    func loadRegistered(_ id: String) async {
        guard !busyModelIDs.contains(id) else { return }
        busyModelIDs.insert(id)
        defer { busyModelIDs.remove(id) }
        do {
            _ = try await api.loadRegistered(id)
            logInfo("已加载模型：\(id)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "加载失败：\(Self.message(for: error))"
        }
    }

    func unload(_ id: String) async {
        guard !busyModelIDs.contains(id) else { return }
        busyModelIDs.insert(id)
        defer { busyModelIDs.remove(id) }
        do {
            _ = try await api.unload(id)
            logInfo("已停止模型：\(id)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "卸载失败：\(Self.message(for: error))"
        }
    }

    /// 从注册表删除模型（已加载时 daemon 会先卸载 worker），并同步本地上下文设置。
    func unregister(_ id: String) async {
        guard !busyModelIDs.contains(id) else { return }
        busyModelIDs.insert(id)
        defer { busyModelIDs.remove(id) }
        do {
            try await api.unregister(id)
            ModelRepository.removeContextLength(for: id)
            logInfo("已删除模型注册：\(id)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "删除失败：\(Self.message(for: error))"
        }
    }

    /// 重命名模型 ID（daemon 端保留全部策略设置）。
    func rename(_ id: String, to newID: String) async {
        guard !busyModelIDs.contains(id) else { return }
        busyModelIDs.insert(id)
        defer { busyModelIDs.remove(id) }
        do {
            try await api.rename(id, to: newID)
            // 本地按旧 ID 存的上下文设置迁移到新 ID。
            if let length = ModelRepository.customContextLength(for: id) {
                ModelRepository.setContextLength(length, for: newID)
                ModelRepository.removeContextLength(for: id)
            }
            logInfo("已重命名模型：\(id) → \(newID)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "重命名失败：\(Self.message(for: error))"
        }
    }

    /// 调整 keep_alive 策略：只改注册表，进程保持常驻，reaper 按新值执行。
    func setKeepAlive(_ id: String, keepAlive: String?) async {
        do {
            try await api.setKeepAlive(id, keepAlive: keepAlive)
            logInfo("已更新模型驻留策略：\(id)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "策略修改失败：\(Self.message(for: error))"
        }
    }

    /// 修改 TTS 默认音色，只改策略，不重载 worker。
    func setVoice(_ id: String, voice: String) async {
        do {
            try await api.setVoice(id, voice: voice)
            logInfo("已更新 TTS 默认音色：\(id) / \(voice)")
            await refreshAfterSuccessfulMutation()
        } catch {
            lastError = "音色修改失败：\(Self.message(for: error))"
        }
    }

    /// 获取 TTS 模型的音色列表。
    func voices(for id: String) async throws -> VoiceResponse {
        try await api.voices(id)
    }

    func registerAndLoad(path: String, id: String, contextLength: Int, keepAlive: String?, modelType: String? = nil, provider: String? = nil) async throws {
        _ = try await api.registerAndLoad(path: path, id: id, name: nil, contextLength: contextLength, keepAlive: keepAlive, modelType: modelType, provider: provider)
        logInfo("已注册并加载模型：\(id)")
        await refreshAfterSuccessfulMutation()
    }

    func providerIsAvailable(_ providerID: String) -> Bool {
        providers.first { $0.descriptor.id == providerID }?.status.available == true
    }

    /// 推荐模型的一键流程：下载 → 注册 → 启动。Provider 环境未就绪时先完成下载，
    /// 保留本地模型，等环境可用后再次点击即可注册启动。
    @discardableResult
    func installRecommended(_ model: RecommendedModel) async -> Bool {
        guard !busyRecommendationIDs.contains(model.id) else { return false }
        busyRecommendationIDs.insert(model.id)
        defer { busyRecommendationIDs.remove(model.id) }
        lastError = nil
        do {
            let isLoaded = info?.loadedModels.contains { $0.id == model.id } == true
            if !model.isDownloaded {
                // 已驻留模型补充新增清单文件时只下载，避免无意义重启；首次下载则直接注册启动。
                let autoLoad = providerIsAvailable(model.provider) && !isLoaded
                _ = try await api.pull(model, autoLoad: autoLoad)
                logInfo(autoLoad
                    ? "已下载并启动推荐模型：\(model.id)"
                    : "已补全推荐模型文件：\(model.id)")
            } else if isLoaded {
                return true
            } else if registeredModels.contains(where: { $0.id == model.id }) {
                _ = try await api.loadRegistered(model.id)
                logInfo("已启动推荐模型：\(model.id)")
            } else {
                _ = try await api.registerAndLoad(
                    path: model.downloadedURL?.path ?? model.localURL.path,
                    id: model.id,
                    name: model.title,
                    contextLength: nil,
                    keepAlive: nil,
                    modelType: model.modelType,
                    provider: model.provider
                )
                logInfo("已注册并启动推荐模型：\(model.id)")
            }
            await refreshAfterSuccessfulMutation()
            return true
        } catch {
            lastError = "推荐模型操作失败：\(Self.message(for: error))"
            return false
        }
    }

    func setLogLevel(_ level: LogLevel) async throws {
        guard phase == .online else {
            logInfo("日志级别将在下次启动 aiworkd 时应用：\(level.title)")
            return
        }
        let response = try await api.setLogLevel(level)
        logInfo("日志级别已切换为 \(response.level.capitalized)")
    }

    // MARK: - 轮询内核

    /// 执行一个轮询周期；保持 internal 以便用受控 HTTP 响应验证状态转换。
    func tick() async -> Double {
        switch phase {
        case .offline:
            await connectIfHealthy(message: "已连接 aiworkd")
            return 1.0

        case .starting:
            await connectIfHealthy(message: "aiworkd 已就绪")
            return 0.4

        case .stopping:
            if !(await api.isHealthy()) {
                let shouldRestart = restartAfterStop
                finishStop()
                if shouldRestart {
                    restartAfterStop = false
                    startDaemon()
                    return 0.4
                }
                return 1.0
            }
            if let deadline = stopDeadline, Date.now >= deadline {
                if let pid = info?.pid, pid > 0 {
                    logWarning("aiworkd 未在期限内退出，发送 SIGKILL，PID \(pid)")
                    kill(pid_t(pid), SIGKILL)
                }
                stopDeadline = Date.now.addingTimeInterval(4)
            }
            return 0.4

        case .online:
            do {
                try await refresh()
                recordRefreshSuccess()
            } catch {
                if await daemonIsConfirmedOffline() {
                    lastError = "与守护进程的连接中断：\(Self.message(for: error))"
                    finishStop()
                } else {
                    recordRefreshFailure(error)
                }
            }
            return 2.0
        }
    }

    private func finishStop() {
        logInfo("aiworkd 当前离线")
        phase = .offline
        info = nil
        registeredModels = []
        providers = []
        runningTasks = []
        completedTasks = []
        tasksError = nil
        childProcess = nil
        stopDeadline = nil
        refreshIsDegraded = false
        try? logHandle?.close()
        logHandle = nil
    }

    private func connectIfHealthy(message: String) async {
        guard await api.isHealthy() else { return }

        phase = .online
        lastError = nil
        do {
            try await refresh()
            recordRefreshSuccess()
            if let pid = info?.pid, pid > 0 {
                logInfo("\(message)，PID \(pid)")
            } else {
                logInfo(message)
            }
        } catch {
            recordRefreshFailure(error)
            logInfo("\(message)，运行状态暂不可读")
        }
        try? await setLogLevel(AppSettings.logLevel)
    }

    private func refreshAfterSuccessfulMutation() async {
        do {
            try await refresh()
            recordRefreshSuccess()
        } catch {
            recordRefreshFailure(error)
        }
    }

    private func daemonIsConfirmedOffline() async -> Bool {
        guard !(await api.isHealthy()) else { return false }
        try? await Task.sleep(for: .milliseconds(250))
        return !(await api.isHealthy())
    }

    private func recordRefreshFailure(_ error: Error) {
        guard !refreshIsDegraded else { return }
        refreshIsDegraded = true
        logWarning("aiworkd 仍在线，运行状态刷新暂时失败：\(Self.message(for: error))")
    }

    private func recordRefreshSuccess() {
        guard refreshIsDegraded else { return }
        refreshIsDegraded = false
        logInfo("aiworkd 运行状态刷新已恢复")
    }

    // MARK: - 子进程

    static func environmentByApplyingSystemProxy(
        _ settings: [String: Any],
        to base: [String: String]
    ) -> [String: String] {
        var environment = base

        func proxyURL(enable: String, host: String, port: String) -> String? {
            guard (settings[enable] as? NSNumber)?.boolValue == true,
                  let hostname = settings[host] as? String,
                  !hostname.isEmpty,
                  let portNumber = settings[port] as? NSNumber else { return nil }
            return "http://\(hostname):\(portNumber.intValue)"
        }

        if environment["HTTP_PROXY"] == nil,
           environment["http_proxy"] == nil,
           let proxy = proxyURL(enable: "HTTPEnable", host: "HTTPProxy", port: "HTTPPort") {
            environment["HTTP_PROXY"] = proxy
        }
        if environment["HTTPS_PROXY"] == nil,
           environment["https_proxy"] == nil,
           let proxy = proxyURL(enable: "HTTPSEnable", host: "HTTPSProxy", port: "HTTPSPort") {
            environment["HTTPS_PROXY"] = proxy
        }

        return environmentByAddingLocalProxyExclusions(environment)
    }

    static func environmentByApplyingProxyMode(
        _ mode: ProxyMode,
        httpProxy: String?,
        httpsProxy: String?,
        systemSettings: [String: Any]?,
        to base: [String: String]
    ) -> [String: String] {
        switch mode {
        case .system:
            guard let systemSettings else { return base }
            return environmentByApplyingSystemProxy(systemSettings, to: base)
        case .disabled:
            var environment = base
            for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
                environment.removeValue(forKey: key)
            }
            return environment
        case .manual:
            var environment = environmentByApplyingProxyMode(
                .disabled,
                httpProxy: nil,
                httpsProxy: nil,
                systemSettings: nil,
                to: base
            )
            if let httpProxy { environment["HTTP_PROXY"] = httpProxy }
            if let httpsProxy { environment["HTTPS_PROXY"] = httpsProxy }
            return environmentByAddingLocalProxyExclusions(environment)
        }
    }

    private static func environmentByAddingLocalProxyExclusions(
        _ base: [String: String]
    ) -> [String: String] {
        var environment = base
        let localHosts = ["127.0.0.1", "localhost", "::1"]
        var exclusions = (environment["NO_PROXY"] ?? environment["no_proxy"] ?? "")
            .split(separator: ",")
            .map(String.init)
        for host in localHosts where !exclusions.contains(host) {
            exclusions.append(host)
        }
        environment["NO_PROXY"] = exclusions.joined(separator: ",")
        return environment
    }

    private static func spawn(binary: URL) throws -> (Process, FileHandle) {
        let process = Process()
        process.executableURL = binary

        let systemSettings = SCDynamicStoreCopyProxies(nil) as? [String: Any]
        var environment = environmentByApplyingProxyMode(
            AppSettings.proxyMode,
            httpProxy: AppSettings.httpProxy,
            httpsProxy: AppSettings.httpsProxy,
            systemSettings: systemSettings,
            to: ProcessInfo.processInfo.environment
        )
        environment["RUST_LOG"] = AppSettings.logLevel.rawValue
        if let budget = AppSettings.parseMemoryBudget(AppSettings.memoryBudgetText) {
            environment["AIWORKD_MEMORY_BUDGET"] = String(budget)
        } else {
            environment.removeValue(forKey: "AIWORKD_MEMORY_BUDGET")
        }
        environment["AIWORK_QWEN3_ASR_ENABLED"] = AppSettings.qwen3ASR06BEnabled ? "1" : "0"
        process.environment = environment

        // binary 位于 <仓库>/target/<配置>/aiworkd，工作目录定为仓库根
        var cwd = binary.deletingLastPathComponent()
        cwd = cwd.deletingLastPathComponent()
        cwd = cwd.deletingLastPathComponent()
        process.currentDirectoryURL = cwd

        let logURL = LogFiles.daemon
        let handle = try LogFiles.openForAppend(at: logURL)
        let marker = "\n--- \(Date().ISO8601Format()) GUI 启动 aiworkd ---\n"
        try handle.write(contentsOf: Data(marker.utf8))
        process.standardOutput = handle
        process.standardError = handle
        try process.run()
        return (process, handle)
    }

    static var logDirectory: URL {
        LogFiles.directory
    }

    private func logInfo(_ message: String) {
        guard logsEnabled else { return }
        AppLogger.info(message)
    }

    private func logWarning(_ message: String) {
        guard logsEnabled else { return }
        AppLogger.warning(message)
    }

    private func logError(_ message: String) {
        guard logsEnabled else { return }
        AppLogger.error(message)
    }

    /// 二进制探测顺序：AIWORKD_PATH 环境变量 → 仓库 target/release、target/debug。
    static func resolveBinary() -> URL? {
        var candidates: [String] = []
        if let envPath = ProcessInfo.processInfo.environment["AIWORKD_PATH"] {
            candidates.append(envPath)
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let repo = home.appendingPathComponent("Projects/MacAI")
        candidates.append(repo.appendingPathComponent("target/release/aiworkd").path)
        candidates.append(repo.appendingPathComponent("target/debug/aiworkd").path)
        for candidate in candidates where FileManager.default.isExecutableFile(atPath: candidate) {
            return URL(fileURLWithPath: candidate)
        }
        return nil
    }

    static func message(for error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}
