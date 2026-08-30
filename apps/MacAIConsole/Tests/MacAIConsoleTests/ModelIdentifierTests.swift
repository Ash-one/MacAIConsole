import Foundation
import XCTest
@testable import MacAIConsole

final class ModelIdentifierTests: XCTestCase {
    func testFileModelIDRemovesOnlyLastExtensionAndDirectoryIDIsPreserved() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("macai-model-id-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        let file = root.appendingPathComponent("whisper.large-v3.q5_0.bin")
        try Data().write(to: file)
        let fileModel = RepoModel(
            fileName: file.lastPathComponent,
            modelType: "stt",
            provider: "whisper.cpp",
            path: file.path,
            sizeBytes: 0
        )
        XCTAssertEqual(fileModel.modelID, "whisper.large-v3.q5_0")

        let directory = root.appendingPathComponent("Qwen3-ASR-0.6B-MLX-8bit", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let directoryModel = RepoModel(
            fileName: directory.lastPathComponent,
            modelType: "stt",
            provider: "qwen3-asr-mlx",
            path: directory.path,
            sizeBytes: 0
        )
        XCTAssertEqual(directoryModel.modelID, "Qwen3-ASR-0.6B-MLX-8bit")

        let registered = ModelEntry(
            id: "Qwen3-ASR-0",
            ownedBy: "aiworkd/qwen3-asr-mlx",
            modelType: "stt",
            path: directory.path
        )
        XCTAssertEqual(registered.originalID, "Qwen3-ASR-0.6B-MLX-8bit")
    }
}
