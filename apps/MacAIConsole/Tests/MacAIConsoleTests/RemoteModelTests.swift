#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class RemoteModelTests: XCTestCase {
    func testParsesBareIDsAndKnownModelURLs() throws {
        XCTAssertEqual(
            try RemoteModelReference.parse("owner/model", selectedSource: .huggingface),
            RemoteModelReference(source: .huggingface, repo: "owner/model")
        )
        XCTAssertEqual(
            try RemoteModelReference.parse("https://huggingface.co/owner/model", selectedSource: .modelscope),
            RemoteModelReference(source: .huggingface, repo: "owner/model")
        )
        XCTAssertEqual(
            try RemoteModelReference.parse("https://modelscope.cn/models/owner/model", selectedSource: .huggingface),
            RemoteModelReference(source: .modelscope, repo: "owner/model")
        )
        XCTAssertThrowsError(try RemoteModelReference.parse("model", selectedSource: .huggingface))
        XCTAssertThrowsError(try RemoteModelReference.parse("https://example.com/owner/model", selectedSource: .huggingface))
    }

    func testSingleFileCandidatesOnlyUseSupportedRepositoryShapes() {
        let files = [
            RemoteModelFile(path: "model-q4.gguf", sizeBytes: 1),
            RemoteModelFile(path: "pytorch_model.bin", sizeBytes: 2),
            RemoteModelFile(path: "ggml-base.bin", sizeBytes: 3),
        ]
        XCTAssertEqual(
            RemoteModelSelection.singleFileCandidates(modelType: "llm", files: files).map(\.path),
            ["model-q4.gguf"]
        )
        XCTAssertEqual(
            RemoteModelSelection.singleFileCandidates(modelType: "stt", files: files).map(\.path),
            ["ggml-base.bin"]
        )
        XCTAssertTrue(RemoteModelSelection.singleFileCandidates(modelType: "tts", files: files).isEmpty)
    }
}
#endif
