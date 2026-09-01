import Foundation
import XCTest
@testable import MacAIConsole

final class GGUFMetadataReaderTests: XCTestCase {
    func testReadsModelIdentityArchitectureContextAndQuantization() throws {
        let url = temporaryURL()
        defer { try? FileManager.default.removeItem(at: url) }

        var data = Data("GGUF".utf8)
        append(UInt32(3), to: &data)
        append(UInt64(335), to: &data)
        append(UInt64(12), to: &data)
        appendString("general.architecture", value: "qwen35", to: &data)
        appendString("general.type", value: "model", to: &data)
        appendString("general.name", value: "Qwen3.8 2B", to: &data)
        appendString("general.basename", value: "Qwen3.8", to: &data)
        appendString("general.size_label", value: "2B", to: &data)
        appendUInt32("qwen35.context_length", value: 262_144, to: &data)
        appendUInt32("qwen35.block_count", value: 25, to: &data)
        appendUInt32("qwen35.embedding_length", value: 2_048, to: &data)
        appendString("tokenizer.ggml.model", value: "gpt2", to: &data)
        appendStringArray("tokenizer.ggml.tokens", values: ["a", "b"], to: &data)
        appendUInt32("general.quantization_version", value: 2, to: &data)
        appendUInt32("general.file_type", value: 15, to: &data)
        try data.write(to: url)

        let metadata = try GGUFMetadataReader.read(from: url)

        XCTAssertEqual(metadata.formatVersion, 3)
        XCTAssertEqual(metadata.tensorCount, 335)
        XCTAssertEqual(metadata.metadataCount, 12)
        XCTAssertEqual(metadata.architecture, "qwen35")
        XCTAssertEqual(metadata.modelType, "model")
        XCTAssertEqual(metadata.name, "Qwen3.8 2B")
        XCTAssertEqual(metadata.baseName, "Qwen3.8")
        XCTAssertEqual(metadata.sizeLabel, "2B")
        XCTAssertEqual(metadata.contextLength, 262_144)
        XCTAssertEqual(metadata.blockCount, 25)
        XCTAssertEqual(metadata.embeddingLength, 2_048)
        XCTAssertEqual(metadata.tokenizerModel, "gpt2")
        XCTAssertEqual(metadata.quantizationVersion, 2)
        XCTAssertEqual(metadata.fileType, 15)
        XCTAssertEqual(metadata.quantizationName, "Q4_K_M")
        XCTAssertEqual(metadata.summary, "qwen35 · Q4_K_M · 原生上下文 256K")
    }

    func testRejectsInvalidMagicBeforeReadingMetadata() throws {
        let url = temporaryURL()
        defer { try? FileManager.default.removeItem(at: url) }
        try Data("NOPE".utf8).write(to: url)

        XCTAssertThrowsError(try GGUFMetadataReader.read(from: url)) { error in
            XCTAssertEqual(error as? GGUFMetadataError, .invalidMagic)
        }
    }

    func testRejectsTruncatedMetadataWithoutReadingTensorPayload() throws {
        let url = temporaryURL()
        defer { try? FileManager.default.removeItem(at: url) }

        var data = Data("GGUF".utf8)
        append(UInt32(3), to: &data)
        append(UInt64(1), to: &data)
        append(UInt64(1), to: &data)
        append(UInt64(64), to: &data)
        try data.write(to: url)

        XCTAssertThrowsError(try GGUFMetadataReader.read(from: url)) { error in
            XCTAssertEqual(error as? GGUFMetadataError, .truncated)
        }
    }

    private func temporaryURL() -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("macai-gguf-\(UUID().uuidString).gguf")
    }

    private func appendString(_ key: String, value: String, to data: inout Data) {
        append(key, to: &data)
        append(UInt32(8), to: &data)
        append(value, to: &data)
    }

    private func appendUInt32(_ key: String, value: UInt32, to data: inout Data) {
        append(key, to: &data)
        append(UInt32(4), to: &data)
        append(value, to: &data)
    }

    private func appendStringArray(_ key: String, values: [String], to data: inout Data) {
        append(key, to: &data)
        append(UInt32(9), to: &data)
        append(UInt32(8), to: &data)
        append(UInt64(values.count), to: &data)
        for value in values {
            append(value, to: &data)
        }
    }

    private func append(_ value: String, to data: inout Data) {
        let bytes = Data(value.utf8)
        append(UInt64(bytes.count), to: &data)
        data.append(bytes)
    }

    private func append<T: FixedWidthInteger>(_ value: T, to data: inout Data) {
        var littleEndian = value.littleEndian
        Swift.withUnsafeBytes(of: &littleEndian) { bytes in
            data.append(contentsOf: bytes)
        }
    }
}
