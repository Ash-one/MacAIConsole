import Foundation
import Observation
import SystemConfiguration

/// Provider worker 所需的运行环境（venv 或编译产物）定义。
/// 路径与依赖版本对应 README：daemon 按仓库根 `.build/` 下的固定布局探测
/// worker 解释器 / 引擎二进制，GUI 安装必须落到同一路径。
struct PythonEnvironmentSpec: Identifiable, Equatable {
    /// 与 daemon 的 provider id 一致，便于从模型页反查。
    let id: String
    let label: String
    let summary: String
    let venvName: String
    /// 固定版本依赖；与 README 保持一致，避免自动安装装出不兼容组合。
    let packages: [String]
    /// daemon 侧对应的解释器覆盖环境变量；用户设置了它时 GUI 不再判定/安装本地环境。
    let pythonOverrideEnv: String?
    /// 脚本安装型环境（如 llama.cpp）：设置后安装执行 `bash <脚本>`，忽略 venv/pip 流程。
    var installScript: String? = nil
    /// 安装产物相对路径覆盖；缺省为 venv 解释器 `.build/<venvName>/bin/python`。
    var artifactPath: String? = nil

    var venvRelativePath: String { ".build/\(venvName)" }

    /// 安装完成的判定产物（仓库根相对路径）：venv 环境是解释器，脚本环境是编译产物。
    var artifactRelativePath: String {
        artifactPath ?? ".build/\(venvName)/bin/python"
    }

    static let sherpaOnnx = PythonEnvironmentSpec(
        id: "sherpa-onnx",
        label: "STT · sherpa-onnx",
        summary: "中文语音识别（Zipformer int8，CPU 或 Core ML）",
        venvName: "sherpa-onnx-venv",
        packages: ["sherpa-onnx==1.13.6", "numpy>=1.26,<3"],
        pythonOverrideEnv: "AIWORK_SHERPA_ONNX_PYTHON"
    )

    /// llama.cpp 引擎二进制：安装 = 执行仓库脚本（浅克隆固定 revision + cmake 编译
    /// llama-server），产物落 `.build/llama.cpp/bin/llama-server`，与 daemon 探测路径一致。
    static let llamaCpp = PythonEnvironmentSpec(
        id: "llama.cpp",
        label: "LLM · llama.cpp",
        summary: "GGUF 模型推理引擎（Apple Silicon Metal 加速）",
        venvName: "llama.cpp",
        packages: [],
        pythonOverrideEnv: "AIWORK_LLAMA_SERVER",
        installScript: "scripts/build-llama-server.sh",
        artifactPath: ".build/llama.cpp/bin/llama-server"
    )

    static let all = [llamaCpp, sherpaOnnx]

    static func spec(forProvider providerID: String) -> PythonEnvironmentSpec? {
        all.first { $0.id == providerID }
    }
}

/// 运行环境的安装状态机：venv 型走探测解释器 → 创建 venv → pip 安装固定依赖；
/// 脚本型（llama.cpp）直接执行仓库构建脚本。
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

    /// 环境是否就绪。用户用覆盖环境变量把 daemon 指向了其他安装时，
    /// 本地产物存在与否已不影响 daemon，视为就绪。
    func isInstalled(_ spec: PythonEnvironmentSpec) -> Bool {
        if let override = spec.pythonOverrideEnv,
           let path = ProcessInfo.processInfo.environment[override],
           !path.trimmingCharacters(in: .whitespaces).isEmpty {
            return true
        }
        guard let repoRoot else { return false }
        return FileManager.default.isExecutableFile(
            atPath: repoRoot.appendingPathComponent(spec.artifactRelativePath).path
        )
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
            if let script = spec.installScript {
                // 脚本安装型（如 llama.cpp）：脚本自带探测/克隆/增量编译逻辑，可直接重跑。
                update(spec) {
                    $0.phase = .installingPackages
                    $0.stepText = "正在构建（克隆源码并编译，首次可达 10 分钟以上）…"
                }
                try await runProcess(
                    spec,
                    executable: "/bin/bash",
                    arguments: [script],
                    repoRoot: repoRoot
                )
            } else {
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
            }

            update(spec) {
                $0.phase = .idle
                $0.stepText = ""
            }
            AppLogger.info("运行环境安装完成：\(spec.label)")
        } catch is CancellationError {
            update(spec) {
                $0.phase = .idle
                $0.stepText = "已取消"
            }
            AppLogger.info("运行环境安装已取消：\(spec.label)")
        } catch {
            let message = error.localizedDescription
            update(spec) {
                $0.phase = .failed
                $0.stepText = ""
                $0.errorMessage = message
            }
            AppLogger.error("运行环境安装失败（\(spec.label)）：\(message)")
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
