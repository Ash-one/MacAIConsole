#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

@MainActor
final class AppTerminationTests: XCTestCase {
    func testTerminationAlertContentWhenAutoStopEnabled() {
        let dummyInfo = RuntimeInfo(
            version: "0.1.0",
            pid: 12345,
            uptimeSecs: 3600,
            activeRequests: 0,
            loadedModels: [
                LoadedModel(id: "qwen2.5-coder-7b", provider: "org.macai.llama", state: "ready")
            ],
            memoryBudget: 16 * 1024 * 1024 * 1024,
            memoryTotal: 32 * 1024 * 1024 * 1024,
            memoryUsed: 10 * 1024 * 1024 * 1024
        )

        let content = AppDelegate.terminationAlertContent(
            phase: .online,
            info: dummyInfo,
            autoStop: true
        )

        XCTAssertEqual(content.title, "退出 MacAIConsole")
        XCTAssertTrue(content.informative.contains("PID 12345"))
        XCTAssertTrue(content.informative.contains("已加载 1 个模型"))
        XCTAssertTrue(content.informative.contains("当前已开启「退出应用时自动退出守护进程」"))
        // 开启自动关闭守护进程时，仅显示两个选项：关闭所有、取消
        XCTAssertEqual(content.buttons, ["关闭所有", "取消"])
    }

    func testTerminationAlertContentWhenAutoStopDisabled() {
        let content = AppDelegate.terminationAlertContent(
            phase: .online,
            info: nil,
            autoStop: false
        )

        XCTAssertEqual(content.title, "退出 MacAIConsole")
        XCTAssertTrue(content.informative.contains("当前未开启「退出应用时自动退出守护进程」"))
        // 未开启自动关闭守护进程时，显示三个选项：关闭所有、关闭GUI、取消
        XCTAssertEqual(content.buttons, ["关闭所有", "关闭GUI", "取消"])
    }

    func testTerminationAlertContentForOfflineAndTransitionPhases() {
        let offlineContent = AppDelegate.terminationAlertContent(
            phase: .offline,
            info: nil,
            autoStop: true
        )
        XCTAssertTrue(offlineContent.informative.contains("当前未运行"))
        XCTAssertEqual(offlineContent.buttons, ["关闭所有", "取消"])

        let startingContent = AppDelegate.terminationAlertContent(
            phase: .starting,
            info: nil,
            autoStop: false
        )
        XCTAssertTrue(startingContent.informative.contains("正在启动中"))
        XCTAssertEqual(startingContent.buttons, ["关闭所有", "关闭GUI", "取消"])
    }

    func testStopDaemonForExitRespectsForceStopFlag() {
        let controller = DaemonController(logsEnabled: false)

        // forceStop: false 时，即使 phase 是 offline 也能安全返回，不抛异常
        controller.stopDaemonForExit(forceStop: false)

        // forceStop: true 时，没有正在运行的进程也是安全的 no-op
        controller.stopDaemonForExit(forceStop: true)

        let suiteName = "MacAIConsoleTests.Termination.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.set(false, forKey: AppSettings.autoStopOnExitKey)
        defer { defaults.removePersistentDomain(forName: suiteName) }

        // forceStop 为 nil 时，依照 defaults
        controller.stopDaemonForExit(forceStop: nil, defaults: defaults)
    }
}
#endif
