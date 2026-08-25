import Darwin
import Foundation
import Observation

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
    var lastError: String?
    var busyModelIDs: Set<String> = []

    let api = DaemonAPI()

    private var loopTask: Task<Void, Never>?
    private var childProcess: Process?
    private var logHandle: FileHandle?
    private var stopDeadline: Date?
    private var restartAfterStop = false
    // MARK: - 生命周期

    func bootstrapIfNeeded() {
        guard loopTask == nil else { return }
        loopTask = Task { [weak self] in
            while !Task.isCancelled {
                let interval = await self?.tick() ?? 1.0
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
        guard AppSettings.autoStartDaemon else { return }
        Task { [weak self] in
            guard let self, !(await self.api.isHealthy()) else { return }
            self.startDaemon()
        }
    }

    func startDaemon() {
        guard phase == .offline else { return }
        guard let binary = Self.resolveBinary() else {
            lastError = "未找到 aiworkd 可执行文件。请先在 MacAI 仓库构建（cargo build），或在「设置」中指定路径。"
            return
        }
        lastError = nil
        phase = .starting
        do {
            let (process, log) = try Self.spawn(binary: binary)
            childProcess = process
            logHandle = log
            process.terminationHandler = { [weak self] proc in
                Task { @MainActor in
                    guard let self, self.childProcess === proc, self.phase != .stopping else { return }
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
        stopDeadline = Date().addingTimeInterval(8)
        if let pid = info?.pid, pid > 0 {
            kill(pid_t(pid), SIGTERM)
        } else if let child = childProcess, child.isRunning {
            child.terminate()
        }
    }

    func restartDaemon() {
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
        let info = try await api.runtimeInfo()
        let models = try? await api.models()
        let providers = try? await api.providers()
        self.info = info
        if let models { registeredModels = models }
        if let providers { self.providers = providers }
    }

    func loadRegistered(_ id: String) async {
        guard !busyModelIDs.contains(id) else { return }
        busyModelIDs.insert(id)
        defer { busyModelIDs.remove(id) }
        do {
            _ = try await api.loadRegistered(id)
            try await refresh()
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
            try await refresh()
        } catch {
            lastError = "卸载失败：\(Self.message(for: error))"
        }
    }

    /// 调整 keep_alive 策略：只改注册表，进程保持常驻，reaper 按新值执行。
    func setKeepAlive(_ id: String, keepAlive: String?) async {
        do {
            try await api.setKeepAlive(id, keepAlive: keepAlive)
            try await refresh()
        } catch {
            lastError = "策略修改失败：\(Self.message(for: error))"
        }
    }

    /// 修改 TTS 默认音色，只改策略，不重载 worker。
    func setVoice(_ id: String, voice: String) async {
        do {
            try await api.setVoice(id, voice: voice)
            try await refresh()
        } catch {
            lastError = "音色修改失败：\(Self.message(for: error))"
        }
    }

    /// 获取 TTS 模型的音色列表。
    func voices(for id: String) async throws -> VoiceResponse {
        try await api.voices(id)
    }

    func registerAndLoad(path: String, id: String, contextLength: Int, keepAlive: String?, modelType: String? = nil) async throws {
        _ = try await api.registerAndLoad(path: path, id: id, name: nil, contextLength: contextLength, keepAlive: keepAlive, modelType: modelType)
        try await refresh()
    }

    // MARK: - 轮询内核

    private func tick() async -> Double {
        switch phase {
        case .offline:
            if await api.isHealthy() {
                do { try await refresh() } catch {}
                phase = .online
                lastError = nil
            }
            return 1.0

        case .starting:
            if await api.isHealthy() {
                do { try await refresh() } catch {}
                phase = .online
                lastError = nil
            }
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
            if let deadline = stopDeadline, Date() >= deadline {
                if let pid = info?.pid, pid > 0 { kill(pid_t(pid), SIGKILL) }
                stopDeadline = Date().addingTimeInterval(4)
            }
            return 0.4

        case .online:
            do {
                try await refresh()
            } catch {
                lastError = "与守护进程的连接中断：\(Self.message(for: error))"
                finishStop()
            }
            return 2.0
        }
    }

    private func finishStop() {
        phase = .offline
        info = nil
        registeredModels = []
        providers = []
        childProcess = nil
        stopDeadline = nil
        try? logHandle?.close()
        logHandle = nil
    }

    // MARK: - 子进程

    private static func spawn(binary: URL) throws -> (Process, FileHandle) {
        let process = Process()
        process.executableURL = binary

        var environment = ProcessInfo.processInfo.environment
        environment["RUST_LOG"] = "info"
        if let budget = AppSettings.parseMemoryBudget(AppSettings.memoryBudgetText) {
            environment["AIWORKD_MEMORY_BUDGET"] = String(budget)
        } else {
            environment.removeValue(forKey: "AIWORKD_MEMORY_BUDGET")
        }
        process.environment = environment

        // binary 位于 <仓库>/target/<配置>/aiworkd，工作目录定为仓库根
        var cwd = binary.deletingLastPathComponent()
        cwd = cwd.deletingLastPathComponent()
        cwd = cwd.deletingLastPathComponent()
        process.currentDirectoryURL = cwd

        let logDir = logDirectory
        try FileManager.default.createDirectory(at: logDir, withIntermediateDirectories: true)
        let logURL = logDir.appendingPathComponent("aiworkd.log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        let handle = try FileHandle(forWritingTo: logURL)
        process.standardOutput = handle
        process.standardError = handle
        try process.run()
        return (process, handle)
    }

    static var logDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("MacAIConsole", isDirectory: true)
            .appendingPathComponent("logs", isDirectory: true)
    }

    /// 二进制探测顺序：设置中指定的路径 → AIWORKD_PATH 环境变量 → 仓库 target/release、target/debug。
    static func resolveBinary() -> URL? {
        if let configured = AppSettings.aiworkdPath,
           FileManager.default.isExecutableFile(atPath: configured) {
            return URL(fileURLWithPath: configured)
        }
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