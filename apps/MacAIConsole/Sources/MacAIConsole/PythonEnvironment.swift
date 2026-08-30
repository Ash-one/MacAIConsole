import Foundation
import Observation
import SystemConfiguration

/// Provider worker 所需的 Python 运行环境（venv）定义。
/// 路径与依赖版本对应 README §5/§6：daemon 按仓库根 `.build/<venvName>/bin/python`
/// 探测 worker 解释器，GUI 安装必须落到同一路径。
struct PythonEnvironmentSpec: Identifiable, Equatable {
    /// 与 daemon 的 provider id 一致，便于从模型页反查。
    let id: String
    let label: String
    let summary: String
    let venvName: String
    /// 固定版本依赖；与 README 保持一致，避免自动安装装出不兼容组合。
    let packages: [String]
    /// daemon 侧对应的解释器覆盖环境变量；用户设置了它时 GUI 不再判定/安装本地 venv。
    let pythonOverrideEnv: String?

    var venvRelativePath: String { ".build/\(venvName)" }

    static let qwen3ASRMlx = PythonEnvironmentSpec(
        id: "qwen3-asr-mlx",
        label: "Qwen3-ASR · MLX",
        summary: "语音识别（Apple Silicon Metal 加速）",
        venvName: "qwen3-asr-mlx-venv",
        packages: ["mlx-audio==0.5.0"],
        pythonOverrideEnv: "AIWORK_QWEN3_ASR_MLX_PYTHON"
    )
    static let qwen3ASRPyTorch = PythonEnvironmentSpec(
        id: "qwen3-asr",
        label: "Qwen3-ASR · PyTorch",
        summary: "语音识别（MPS/CPU 兼容回退，依赖体积最大）",
        venvName: "qwen3-asr-venv",
        packages: ["qwen-asr==0.0.6"],
        pythonOverrideEnv: "AIWORK_QWEN3_ASR_PYTHON"
    )
    static let kokoroMlx = PythonEnvironmentSpec(
        id: "kokoro-mlx",
        label: "Kokoro TTS · MLX",
        summary: "语音合成（Apple Silicon Metal 加速）",
        venvName: "kokoro-venv",
        packages: ["mlx-audio", "misaki[zh]", "misaki[en]", "phonemizer-fork", "espeakng-loader"],
        pythonOverrideEnv: "AIWORK_KOKORO_PYTHON"
    )

    static let all = [qwen3ASRMlx, qwen3ASRPyTorch, kokoroMlx]

    static func spec(forProvider providerID: String) -> PythonEnvironmentSpec? {
        all.first { $0.id == providerID }
    }
}

/// Python 运行环境的安装状态机：探测解释器 → 创建 venv → pip 安装固定依赖。
/// 全部是用户显式触发的长任务（联网下载可达 GB 级），带流式输出与取消。
@MainActor
@Observable
final class PythonEnvironmentManager {
    struct InstallState: Equatable {
        enum Phase: Equatable {
            case idle
            case creatingVenv
            case installingPackages
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

    /// venv 解释器是否可用。用户用环境变量把 daemon 指向了其他解释器时，
    /// 本地 venv 存在与否已不影响 daemon，视为就绪。
    func isInstalled(_ spec: PythonEnvironmentSpec) -> Bool {
        if let override = spec.pythonOverrideEnv,
           let path = ProcessInfo.processInfo.environment[override],
           !path.trimmingCharacters(in: .whitespaces).isEmpty {
            return true
        }
        guard let repoRoot else { return false }
        return FileManager.default.isExecutableFile(atPath: Self.venvPythonURL(repoRoot: repoRoot, spec: spec).path)
    }

    func state(for spec: PythonEnvironmentSpec) -> InstallState {
        states[spec.id] ?? InstallState()
    }

    func install(_ spec: PythonEnvironmentSpec) {
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
        states[spec.id] = InstallState(phase: .creatingVenv, stepText: "正在探测 Python 3.12…")
        Task { await runInstall(spec, repoRoot: repoRoot) }
    }

    func cancel(_ spec: PythonEnvironmentSpec) {
        guard runningProcesses[spec.id] != nil else { return }
        cancelledSpecIDs.insert(spec.id)
        runningProcesses[spec.id]?.terminate()
    }

    private func runInstall(_ spec: PythonEnvironmentSpec, repoRoot: URL) async {
        defer {
            runningProcesses[spec.id] = nil
            cancelledSpecIDs.remove(spec.id)
        }
        do {
            guard let python = Self.resolvePythonExecutable() else {
                throw InstallError(
                    "未找到 Python 3.12。请先安装（brew install python@3.12），或在设置中用代理排除后重试。"
                )
            }
            AppLogger.info("开始安装 Python 运行环境 \(spec.label)：python=\(python)")

            let venvPython = Self.venvPythonURL(repoRoot: repoRoot, spec: spec)
            if !FileManager.default.isExecutableFile(atPath: venvPython.path) {
                update(spec) {
                    $0.phase = .creatingVenv
                    $0.stepText = "正在创建虚拟环境…"
                }
                _ = try await runProcess(
                    spec,
                    executable: python,
                    arguments: ["-m", "venv", spec.venvRelativePath],
                    repoRoot: repoRoot
                )
            }

            update(spec) {
                $0.phase = .installingPackages
                $0.stepText = "正在安装依赖（可能需要数分钟，取决于网速）…"
            }
            _ = try await runProcess(
                spec,
                executable: venvPython.path,
                arguments: ["-m", "pip", "install"] + spec.packages,
                repoRoot: repoRoot
            )

            update(spec) {
                $0.phase = .idle
                $0.stepText = ""
            }
            AppLogger.info("Python 运行环境安装完成：\(spec.label)")
        } catch is CancellationError {
            update(spec) {
                $0.phase = .idle
                $0.stepText = "已取消"
            }
            AppLogger.info("Python 运行环境安装已取消：\(spec.label)")
        } catch {
            let message = error.localizedDescription
            update(spec) {
                $0.phase = .failed
                $0.stepText = ""
                $0.errorMessage = message
            }
            AppLogger.error("Python 运行环境安装失败（\(spec.label)）：\(message)")
        }
    }

    // MARK: - 子进程

    private func runProcess(
        _ spec: PythonEnvironmentSpec,
        executable: String,
        arguments: [String],
        repoRoot: URL
    ) async throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.currentDirectoryURL = repoRoot
        process.environment = Self.childEnvironment()

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        // 不接管 stdin：pip / venv 不需要交互，挂起等待会拖住流关闭。
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

        // terminate 触发的退出按取消处理，不当作 pip 失败惊吓用户。
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

    private func appendOutput(_ text: String, for spec: PythonEnvironmentSpec) {
        update(spec) {
            var tail = $0.outputTail + text
            if tail.count > 600 {
                tail = String(tail.suffix(600))
            }
            $0.outputTail = tail
        }
    }

    private func update(_ spec: PythonEnvironmentSpec, _ mutate: (inout InstallState) -> Void) {
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

    static func venvPythonURL(repoRoot: URL, spec: PythonEnvironmentSpec) -> URL {
        repoRoot.appendingPathComponent(spec.venvRelativePath)
            .appendingPathComponent("bin")
            .appendingPathComponent("python")
    }

    /// 解释器探测：只认名字明确的 python3.12（Homebrew 常见位置优先，其次 PATH）。
    /// 系统默认 python3 可能是其他版本，固定依赖按 3.12 锁定，不盲选。
    static func resolvePythonExecutable(
        candidateExists: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> String? {
        let candidates = [
            "/opt/homebrew/bin/python3.12",
            "/usr/local/bin/python3.12",
        ]
        if let found = candidates.first(where: candidateExists) {
            return found
        }
        guard let path = ProcessInfo.processInfo.environment["PATH"] else { return nil }
        for directory in path.split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(directory)).appendingPathComponent("python3.12").path
            if candidateExists(candidate) {
                return candidate
            }
        }
        return nil
    }

    /// pip 需要走 GUI 配置的代理（模型权重下载由 daemon 承担，pip 下载由这里承担）。
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
