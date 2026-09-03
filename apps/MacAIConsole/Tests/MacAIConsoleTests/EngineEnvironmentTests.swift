import Foundation
import XCTest
@testable import MacAIConsole

final class EngineEnvironmentTests: XCTestCase {
    /// GUI 侧引擎安装只剩 llama.cpp（脚本安装型）；whisper.cpp 这类原生
    /// provider 与 org.macai.* Runner（环境由 daemon uv 受管、经 /api/runners
    /// 安装）不应有 GUI 侧 .build 规格。
    @MainActor
    func testRecommendedModelsDoNotMapToEngineInstallExceptLlamaCpp() {
        for model in RecommendedModel.builtIns {
            let spec = EngineEnvironmentSpec.spec(forProvider: model.provider)
            if model.provider == "llama.cpp" {
                XCTAssertNotNil(spec, "\(model.provider) 需要引擎环境规格")
                XCTAssertEqual(spec?.id, model.provider)
            } else {
                XCTAssertNil(spec, "\(model.provider) 不需要 GUI 侧引擎安装")
            }
        }
    }

    @MainActor
    func testLlamaCppArtifactMatchesDaemonProbePath() {
        // llama.cpp 是脚本安装型环境：就绪判定与安装产物必须指向 daemon
        // 的开发态探测路径 .build/llama.cpp/bin/llama-server。
        let spec = EngineEnvironmentSpec.llamaCpp
        XCTAssertEqual(spec.artifactPath, ".build/llama.cpp/bin/llama-server")
        XCTAssertTrue(spec.installScript.hasSuffix("build-llama-server.sh"))
        XCTAssertEqual(spec.overrideEnv, "AIWORK_LLAMA_SERVER")
    }

    @MainActor
    func testRepoRootDerivedFromTargetBinary() {
        let root = EngineEnvironmentManager.repoRoot(
            fromBinary: URL(fileURLWithPath: "/repo/target/release/aiworkd")
        )
        XCTAssertEqual(root.standardizedFileURL.path, "/repo")
    }
}
