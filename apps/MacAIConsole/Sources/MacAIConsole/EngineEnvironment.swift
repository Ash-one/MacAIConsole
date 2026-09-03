import Foundation
import Observation
import SystemConfiguration

/// 需 GUI 侧一键安装的引擎环境定义。Python worker 已全部迁移 Runner
///（环境由 daemon uv 受管，经「Runner 引擎」区块安装），这里只剩
/// llama.cpp 原生二进制的脚本安装。
/// 路径与 README 对应：daemon 按仓库根 `.build/` 下的固定布局探测
/// 引擎二进制，GUI 安装必须落到同一路径。
struct EngineEnvironmentSpec: Identifiable, Equatable {
    /// 与 daemon 的 provider id 一致，便于从模型页反查。
    let id: String
    let label: String
    let summary: String
    /// daemon 侧对应的产物覆盖环境变量；用户设置了它时 daemon 不再使用
    /// 本地安装产物，GUI 的就绪判定也随之视为就绪。
    let overrideEnv: String
    /// 安装执行仓库脚本（浅克隆固定 revision + cmake 编译 llama-server）。
    let installScript: String
    /// 安装产物相对路径（仓库根相对）。
    let artifactPath: String

    /// llama.cpp 引擎二进制：产物落 `.build/llama.cpp/bin/llama-server`，
    /// 与 daemon 探测路径一致。
    static let llamaCpp = EngineEnvironmentSpec(
        id: "llama.cpp",
        label: "LLM · llama.cpp",
        summary: "GGUF 模型推理引擎（Apple Silicon Metal 加速）",
        overrideEnv: "AIWORK_LLAMA_SERVER",
        installScript: "scripts/build-llama-server.sh",
        artifactPath: ".build/llama.cpp/bin/llama-server"
    )

    static let all = [llamaCpp]

    static func spec(forProvider providerID: String) -> EngineEnvironmentSpec? {
        all.first { $0.id == providerID }
    }
}

/// 引擎环境的安装状态机：执行仓库构建脚本，流式输出 + 可取消。
/// 用户显式触发的长任务（联网克隆 + 编译可达 10 分钟以上）。
@MainActor
@Observable
final class EngineEnvironmentManager {
    struct InstallState: Equatable {
        enum Phase: Equatable {
            case idle
            case installing
            case failed
        }

        var phase: Phase = .idle
        /// 安装中的当前步骤说明。
        var stepText: String = ""
        /// 最近若干输出，失败时用于诊断。
        var outputTail: String = ""
        var errorMessage: String?
    }

    private(set) var states: [String: InstallState] = [:]
    /// 每个 spec 至多一个进行中的安装进程，取消时用它 terminate。
    private var runningProcesses: [String: Process] = [:]
    /// 用户主动取消过的 spec；terminate 的非零退出码据此按取消而非失败处理。
    private var cancelledSpecIDs: Set<String> = []

    /// 仓库根目录，与 DaemonController.spawn 推导 aiworkd cwd 的方式一致。
    var repoRoot: URL? {
        Self.resolveRepoRoot()
    }

    /// 环境是否就绪。用户用覆盖环境变量把 daemon 指向了其他安装时，
    /// 本地产物存在与否已不影响 daemon，视为就绪。
    func isInstalled(_ spec: EngineEnvironmentSpec) -> Bool {
        if let path = ProcessInfo.processInfo.environment[spec.overrideEnv],
           !path.trimmingCharacters(in: .whitespaces).isEmpty {
            return true
        }
        guard let repoRoot else { return false }
        return FileManager.default.isExecutableFile(
            atPath: repoRoot.appendingPathComponent(spec.artifactPath).path
        )
    }

    func state(for spec: EngineEnvironmentSpec) -> InstallState {
        states[spec.id] ?? InstallState()
    }

    func install(_ spec: EngineEnvironmentSpec) {
        guard runningProcesses[spec.id] == nil else { return }
        guard let repoRoot else {
            states[spec.id] = InstallState(
                phase: .failed,
                stepText: "",
                outputTail: "",
                errorMessage: "未找到 MacAI 仓库根目录，无法确定 .build/ 路径。请先构建 aiworkd。"
            )
            return
        }
        states[spec.id] = InstallState(phase: .installing, stepText: "正在构建（克隆源码并编译，首次可达 10 分钟以上）…")
        Task { await runInstall(spec, repoRoot: repoRoot) }
    }

    func cancel(_ spec: EngineEnvironmentSpec) {
        guard runningProcesses[spec.id] != nil else { return }
        cancelledSpecIDs.insert(spec.id)
        runningProcesses[spec.id]?.terminate()
    }

    private func runInstall(_ spec: EngineEnvironmentSpec, repoRoot: URL) async {
        defer {
            runningProcesses[spec.id] = nil
            cancelledSpecIDs.remove(spec.id)
        }
        do {
            // 脚本自带探测/克隆/增量编译逻辑，可直接重跑。
            try await runProcess(
                spec,
                executable: "/bin/bash",
                arguments: [spec.installScript],
                repoRoot: repoRoot
            )

            update(spec) {
                $0.phase = .idle
                $0.stepText = ""
            }
            AppLogger.info("引擎环境安装完成：\(spec.label)")
        } catch is CancellationError {
            update(spec) {
                $0.phase = .idle
                $0.stepText = "已取消"
            }
            AppLogger.info("引擎环境安装已取消：\(spec.label)")
        } catch {
            let message = error.localizedDescription
            update(spec) {
                $0.phase = .failed
                $0.stepText = ""
                $0.errorMessage = message
            }
            AppLogger.error("引擎环境安装失败（\(spec.label)）：\(message)")
        }
    }

    // MARK: - 子进程

    private func runProcess(
        _ spec: EngineEnvironmentSpec,
        executable: String,
        arguments: [String],
        repoRoot: URL
    ) async throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.currentDirectoryURL = repoRoot
        // Finder 启动的 GUI 继承的 PATH 不含 Homebrew，而 cmake / git 等构建工具都在那里；
        // 安装子进程统一前置 Homebrew 目录，避免脚本中途 command not found。
        var environment = Self.childEnvironment()
        let currentPath = environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        let existing = Set(currentPath.split(separator: ":").map(String.init))
        let homebrewDirectories = ["/opt/homebrew/bin", "/usr/local/bin"]
            .filter { !existing.contains($0) }
        if !homebrewDirectories.isEmpty {
            environment["PATH"] = (homebrewDirectories + [currentPath]).joined(separator: ":")
        }
        process.environment = environment

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        // 不接管 stdin：构建脚本不需要交互，挂起等待会拖住流关闭。
        process.standardInput = FileHandle.nullDevice

        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            let text = String(data: data, encoding: .utf8) ?? ""
            Task { @MainActor in
                self?.appendOutput(text, for: spec)
            }
        }

        runningProcesses[spec.id] = process
        try process.run()
        let status: Int32 = await withCheckedContinuation { continuation in
            process.terminationHandler = { process in
                continuation.resume(returning: process.terminationStatus)
            }
        }
        pipe.fileHandleForReading.readabilityHandler = nil

        // terminate 触发的退出按取消处理，不当作构建失败惊吓用户。
        if cancelledSpecIDs.contains(spec.id) {
            throw CancellationError()
        }
        guard status == 0 else {
            throw InstallError(
                "命令失败（退出码 \(status)）：\(executable) \(arguments.joined(separator: " "))。详见输出。"
            )
        }
        return status
    }

    private func appendOutput(_ text: String, for spec: EngineEnvironmentSpec) {
        update(spec) {
            var tail = $0.outputTail + text
            if tail.count > 600 {
                tail = String(tail.suffix(600))
            }
            $0.outputTail = tail
        }
    }

    private func update(_ spec: EngineEnvironmentSpec, _ mutate: (inout InstallState) -> Void) {
        var state = states[spec.id] ?? InstallState()
        mutate(&state)
        states[spec.id] = state
    }

    // MARK: - 纯函数（可测试）

    static func resolveRepoRoot() -> URL? {
        guard let binary = DaemonController.resolveBinary() else { return nil }
        return repoRoot(fromBinary: binary)
    }

    /// aiworkd 位于 <仓库>/target/<配置>/aiworkd，向上三级即仓库根。
    static func repoRoot(fromBinary binary: URL) -> URL {
        binary.deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    /// 构建脚本需要走 GUI 配置的代理（模型权重下载由 daemon 承担，
    /// git clone / cmake 拉依赖由这里承担）。
    static func childEnvironment() -> [String: String] {
        let systemSettings = SCDynamicStoreCopyProxies(nil) as? [String: Any]
        return DaemonController.environmentByApplyingProxyMode(
            AppSettings.proxyMode,
            httpProxy: AppSettings.httpProxy,
            httpsProxy: AppSettings.httpsProxy,
            systemSettings: systemSettings,
            to: ProcessInfo.processInfo.environment
        )
    }

    private struct InstallError: LocalizedError {
        let errorDescription: String?

        init(_ message: String) {
            errorDescription = message
        }
    }
}
