import Foundation
import XCTest
@testable import MacAIConsole

final class PythonEnvironmentTests: XCTestCase {
    /// 每个依赖 Python 的推荐模型都必须有对应环境规格，否则模型页的
    /// 「安装运行环境」按钮会无声消失；whisper.cpp 这类非 Python Provider 则不应有。
    @MainActor
    func testEveryPythonBackedRecommendedModelHasEnvironmentSpec() {
        for model in RecommendedModel.builtIns {
            let spec = PythonEnvironmentSpec.spec(forProvider: model.provider)
            if model.provider == "whisper.cpp" {
                XCTAssertNil(spec, "\(model.provider) 不需要 Python 环境")
            } else {
                XCTAssertNotNil(spec, "\(model.provider) 缺少 Python 环境规格")
                XCTAssertEqual(spec?.id, model.provider)
            }
        }
    }

    @MainActor
    func testVenvPathsMatchDaemonProbeLayout() {
        let repoRoot = URL(fileURLWithPath: "/repo")
        for spec in PythonEnvironmentSpec.all {
            let python = PythonEnvironmentManager.venvPythonURL(repoRoot: repoRoot, spec: spec)
            XCTAssertEqual(
                python.standardizedFileURL.path,
                "/repo/.build/\(spec.venvName)/bin/python",
                "GUI 安装路径必须与 daemon 的 .build/<venv>/bin/python 探测布局一致"
            )
        }
    }

    @MainActor
    func testRepoRootDerivedFromTargetBinary() {
        let root = PythonEnvironmentManager.repoRoot(
            fromBinary: URL(fileURLWithPath: "/repo/target/release/aiworkd")
        )
        XCTAssertEqual(root.standardizedFileURL.path, "/repo")
    }

    @MainActor
    func testPythonResolutionPrefersHomebrewThenPATHAndFailsClosed() {
        // /opt/homebrew 存在时直接命中，不再翻 PATH。
        var checked: [String] = []
        let homebrew = PythonEnvironmentManager.resolvePythonExecutable { path in
            checked.append(path)
            return path == "/opt/homebrew/bin/python3.12"
        }
        XCTAssertEqual(homebrew, "/opt/homebrew/bin/python3.12")

        // Homebrew 两处都缺时按 PATH 逐目录探测。
        let fromPATH = PythonEnvironmentManager.resolvePythonExecutable(
            candidateExists: { $0 == "/usr/local/bin/python3.12" }
        )
        XCTAssertEqual(fromPATH, "/usr/local/bin/python3.12")

        // 完全没有 3.12 时返回 nil（界面据此提示安装），不能退化到未知版本的 python3。
        XCTAssertNil(PythonEnvironmentManager.resolvePythonExecutable(candidateExists: { _ in false }))
    }
}
