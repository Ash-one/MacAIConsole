import Foundation
import XCTest
@testable import MacAIConsole

final class ModelIdentifierTests: XCTestCase {
    func testModelEntryDecodesProviderSelectionAuditFields() throws {
        let data = Data(#"{"id":"local","owned_by":"aiworkd/org.macai.llama.cpp","type":"llm","requested_provider":"llama.cpp","provider_selection_reason":"legacy alias","temperature":1.0,"top_p":0.95,"max_tokens":256}"#.utf8)
        let model = try JSONDecoder().decode(ModelEntry.self, from: data)

        XCTAssertEqual(model.requestedProvider, "llama.cpp")
        XCTAssertEqual(model.providerSelectionReason, "legacy alias")
        XCTAssertEqual(model.temperature, 1.0)
        XCTAssertEqual(model.topP, 0.95)
        XCTAssertEqual(model.maxTokens, 256)
    }

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
            path: file.path,
            sizeBytes: 0
        )
        XCTAssertEqual(fileModel.modelID, "whisper.large-v3.q5_0")

        let directory = root.appendingPathComponent("Qwen3-ASR-0.6B-MLX-4bit", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let directoryModel = RepoModel(
            fileName: directory.lastPathComponent,
            modelType: "stt",
            path: directory.path,
            sizeBytes: 0
        )
        XCTAssertEqual(directoryModel.modelID, "Qwen3-ASR-0.6B-MLX-4bit")

        let registered = ModelEntry(
            id: "Qwen3-ASR-0",
            ownedBy: "aiworkd/org.macai.qwen3-asr",
            modelType: "stt",
            path: directory.path
        )
        XCTAssertEqual(registered.originalID, "Qwen3-ASR-0.6B-MLX-4bit")
    }
}
