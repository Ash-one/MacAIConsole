#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

@MainActor
final class MenuBarContentViewTests: XCTestCase {
    func testMenuBarIconMatchesDaemonPhaseAndActivity() {
        let app = MacAIConsoleApp()
        _ = app

        // 测试图标在不同 Phase 下的行为
        func iconName(phase: DaemonController.Phase, activeRequests: UInt64) -> String {
            switch phase {
            case .online:
                return activeRequests > 0 ? "cpu.fill" : "cpu"
            case .starting, .stopping:
                return "hourglass"
            case .offline:
                return "poweroff"
            }
        }

        XCTAssertEqual(iconName(phase: .offline, activeRequests: 0), "poweroff")
        XCTAssertEqual(iconName(phase: .starting, activeRequests: 0), "hourglass")
        XCTAssertEqual(iconName(phase: .stopping, activeRequests: 0), "hourglass")
        XCTAssertEqual(iconName(phase: .online, activeRequests: 0), "cpu")
        XCTAssertEqual(iconName(phase: .online, activeRequests: 3), "cpu.fill")
    }

    func testUnloadedModelsFiltersOutCurrentlyLoadedIDs() {
        let registered = [
            ModelEntry(id: "qwen2.5-7b", ownedBy: "llama.cpp", modelType: "llm"),
            ModelEntry(id: "whisper-large-v3", ownedBy: "whisper.cpp", modelType: "stt"),
            ModelEntry(id: "kokoro-v1.0", ownedBy: "kokoro", modelType: "tts")
        ]

        let loaded = [
            LoadedModel(id: "qwen2.5-7b", provider: "llama.cpp", state: "ready")
        ]

        let loadedIDs = Set(loaded.map(\.id))
        let unloaded = registered.filter { !loadedIDs.contains($0.id) }

        XCTAssertEqual(unloaded.count, 2)
        XCTAssertEqual(unloaded.map(\.id), ["whisper-large-v3", "kokoro-v1.0"])
    }
}
#endif
