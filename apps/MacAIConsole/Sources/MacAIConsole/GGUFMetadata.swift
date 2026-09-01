import Foundation

struct GGUFMetadata: Hashable, Sendable {
    let formatVersion: UInt32
    let tensorCount: UInt64
    let metadataCount: UInt64
    let architecture: String
    let modelType: String?
    let name: String?
    let baseName: String?
    let sizeLabel: String?
    let contextLength: Int?
    let blockCount: Int?
    let embeddingLength: Int?
    let tokenizerModel: String?
    let quantizationVersion: UInt32?
    let fileType: UInt32?

    var quantizationName: String? {
        guard let fileType else { return nil }
        return Self.fileTypeNames[fileType] ?? "类型 \(fileType)"
    }

    var summary: String {
        var parts = [architecture]
        if let quantizationName { parts.append(quantizationName) }
        if let contextLength {
            parts.append("原生上下文 \(Self.formatTokenCount(contextLength))")
        }
        return parts.joined(separator: " · ")
    }

    static func formatTokenCount(_ value: Int) -> String {
        if value >= 1024, value.isMultiple(of: 1024) {
            return "\(value / 1024)K"
        }
        return "\(value) tokens"
    }

    private static let fileTypeNames: [UInt32: String] = [
        0: "F32",
        1: "F16",
        2: "Q4_0",
        3: "Q4_1",
        7: "Q8_0",
        8: "Q5_0",
        9: "Q5_1",
        10: "Q2_K",
        11: "Q3_K_S",
        12: "Q3_K_M",
        13: "Q3_K_L",
        14: "Q4_K_S",
        15: "Q4_K_M",
        16: "Q5_K_S",
        17: "Q5_K_M",
        18: "Q6_K",
    ]
}

enum GGUFMetadataReader {
    static func read(from url: URL) throws -> GGUFMetadata {
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .alwaysMapped)
        } catch {
            throw GGUFMetadataError.unreadable(error.localizedDescription)
        }

        return try data.withUnsafeBytes { bytes in
            var reader = GGUFReader(bytes: bytes)
            return try reader.readMetadata()
        }
    }
}

enum GGUFMetadataError: LocalizedError, Equatable {
    case unreadable(String)
    case invalidMagic
    case unsupportedVersion(UInt32)
    case invalidMetadataCount(UInt64)
    case invalidArrayLength(UInt64)
    case invalidValueType(UInt32)
    case invalidString
    case invalidNumericValue(String)
    case truncated

    var errorDescription: String? {
        switch self {
        case let .unreadable(message):
            "无法读取文件：\(message)"
        case .invalidMagic:
            "文件头不是 GGUF"
        case let .unsupportedVersion(version):
            "不支持 GGUF v\(version)"
        case .invalidMetadataCount:
            "metadata 数量异常"
        case .invalidArrayLength:
            "metadata 数组长度异常"
        case let .invalidValueType(type):
            "未知 metadata 类型 \(type)"
        case .invalidString:
            "metadata 包含无效字符串"
        case let .invalidNumericValue(key):
            "字段 \(key) 的数值无效"
        case .truncated:
            "文件头或 metadata 不完整"
        }
    }
}

private struct GGUFReader {
    private static let maximumMetadataCount: UInt64 = 65_536
    private static let maximumArrayLength: UInt64 = 10_000_000

    let bytes: UnsafeRawBufferPointer
    var offset = 0

    mutating func readMetadata() throws -> GGUFMetadata {
        guard try readBytes(count: 4).elementsEqual([0x47, 0x47, 0x55, 0x46]) else {
            throw GGUFMetadataError.invalidMagic
        }

        let version = try readUInt32()
        guard (2...3).contains(version) else {
            throw GGUFMetadataError.unsupportedVersion(version)
        }

        let tensorCount = try readUInt64()
        let metadataCount = try readUInt64()
        guard metadataCount <= Self.maximumMetadataCount else {
            throw GGUFMetadataError.invalidMetadataCount(metadataCount)
        }

        var architecture: String?
        var modelType: String?
        var name: String?
        var baseName: String?
        var sizeLabel: String?
        var architectureValues: [String: Int] = [:]
        var tokenizerModel: String?
        var quantizationVersion: UInt32?
        var fileType: UInt32?

        for _ in 0..<metadataCount {
            let key = try readString()
            let rawType = try readUInt32()
            guard let type = GGUFValueType(rawValue: rawType) else {
                throw GGUFMetadataError.invalidValueType(rawType)
            }

            switch key {
            case "general.architecture":
                architecture = try readStringValue(type: type)
            case "general.type":
                modelType = try readStringValue(type: type)
            case "general.name":
                name = try readStringValue(type: type)
            case "general.basename":
                baseName = try readStringValue(type: type)
            case "general.size_label":
                sizeLabel = try readStringValue(type: type)
            case "general.quantization_version":
                quantizationVersion = try readUInt32Value(type: type, key: key)
            case "general.file_type":
                fileType = try readUInt32Value(type: type, key: key)
            case "tokenizer.ggml.model":
                tokenizerModel = try readStringValue(type: type)
            default:
                if key.hasSuffix(".context_length")
                    || key.hasSuffix(".block_count")
                    || key.hasSuffix(".embedding_length") {
                    architectureValues[key] = try readIntValue(type: type, key: key)
                } else {
                    try skipValue(type: type)
                }
            }
        }

        guard let architecture, !architecture.isEmpty else {
            throw GGUFMetadataError.invalidString
        }
        let architecturePrefix = "\(architecture)."

        return GGUFMetadata(
            formatVersion: version,
            tensorCount: tensorCount,
            metadataCount: metadataCount,
            architecture: architecture,
            modelType: modelType,
            name: name,
            baseName: baseName,
            sizeLabel: sizeLabel,
            contextLength: architectureValues[architecturePrefix + "context_length"],
            blockCount: architectureValues[architecturePrefix + "block_count"],
            embeddingLength: architectureValues[architecturePrefix + "embedding_length"],
            tokenizerModel: tokenizerModel,
            quantizationVersion: quantizationVersion,
            fileType: fileType
        )
    }

    private mutating func readStringValue(type: GGUFValueType) throws -> String {
        guard type == .string else {
            try skipValue(type: type)
            throw GGUFMetadataError.invalidString
        }
        return try readString()
    }

    private mutating func readUInt32Value(type: GGUFValueType, key: String) throws -> UInt32 {
        let value = try readUnsignedInteger(type: type, key: key)
        guard let result = UInt32(exactly: value) else {
            throw GGUFMetadataError.invalidNumericValue(key)
        }
        return result
    }

    private mutating func readIntValue(type: GGUFValueType, key: String) throws -> Int {
        let value = try readUnsignedInteger(type: type, key: key)
        guard let result = Int(exactly: value) else {
            throw GGUFMetadataError.invalidNumericValue(key)
        }
        return result
    }

    private mutating func readUnsignedInteger(type: GGUFValueType, key: String) throws -> UInt64 {
        switch type {
        case .uint8:
            return UInt64(try readUInt8())
        case .int8:
            let value = try readInt8()
            guard value >= 0 else { throw GGUFMetadataError.invalidNumericValue(key) }
            return UInt64(value)
        case .uint16:
            return UInt64(try readUInt16())
        case .int16:
            let value = try readInt16()
            guard value >= 0 else { throw GGUFMetadataError.invalidNumericValue(key) }
            return UInt64(value)
        case .uint32:
            return UInt64(try readUInt32())
        case .int32:
            let value = try readInt32()
            guard value >= 0 else { throw GGUFMetadataError.invalidNumericValue(key) }
            return UInt64(value)
        case .uint64:
            return try readUInt64()
        case .int64:
            let value = try readInt64()
            guard value >= 0 else { throw GGUFMetadataError.invalidNumericValue(key) }
            return UInt64(value)
        default:
            try skipValue(type: type)
            throw GGUFMetadataError.invalidNumericValue(key)
        }
    }

    private mutating func skipValue(type: GGUFValueType) throws {
        switch type {
        case .uint8, .int8, .bool:
            try skip(count: 1)
        case .uint16, .int16:
            try skip(count: 2)
        case .uint32, .int32, .float32:
            try skip(count: 4)
        case .uint64, .int64, .float64:
            try skip(count: 8)
        case .string:
            try skipString()
        case .array:
            let rawElementType = try readUInt32()
            guard let elementType = GGUFValueType(rawValue: rawElementType) else {
                throw GGUFMetadataError.invalidValueType(rawElementType)
            }
            let count = try readUInt64()
            guard count <= Self.maximumArrayLength else {
                throw GGUFMetadataError.invalidArrayLength(count)
            }
            if let width = elementType.fixedWidth {
                let (byteCount, overflow) = count.multipliedReportingOverflow(by: UInt64(width))
                guard !overflow, let byteCount = Int(exactly: byteCount) else {
                    throw GGUFMetadataError.invalidArrayLength(count)
                }
                try skip(count: byteCount)
            } else {
                for _ in 0..<count {
                    try skipValue(type: elementType)
                }
            }
        }
    }

    private mutating func skipString() throws {
        let count = try readUInt64()
        guard let count = Int(exactly: count) else {
            throw GGUFMetadataError.truncated
        }
        try skip(count: count)
    }

    private mutating func readString() throws -> String {
        let count = try readUInt64()
        guard let count = Int(exactly: count) else {
            throw GGUFMetadataError.truncated
        }
        let value = try readBytes(count: count)
        guard let string = String(bytes: value, encoding: .utf8) else {
            throw GGUFMetadataError.invalidString
        }
        return string
    }

    private mutating func readBytes(count: Int) throws -> UnsafeRawBufferPointer.SubSequence {
        guard count >= 0, offset <= bytes.count, count <= bytes.count - offset else {
            throw GGUFMetadataError.truncated
        }
        let range = offset..<(offset + count)
        offset += count
        return bytes[range]
    }

    private mutating func skip(count: Int) throws {
        _ = try readBytes(count: count)
    }

    private mutating func readUInt8() throws -> UInt8 {
        try readInteger(UInt8.self)
    }

    private mutating func readInt8() throws -> Int8 {
        try readInteger(Int8.self)
    }

    private mutating func readUInt16() throws -> UInt16 {
        try readInteger(UInt16.self)
    }

    private mutating func readInt16() throws -> Int16 {
        try readInteger(Int16.self)
    }

    private mutating func readUInt32() throws -> UInt32 {
        try readInteger(UInt32.self)
    }

    private mutating func readInt32() throws -> Int32 {
        try readInteger(Int32.self)
    }

    private mutating func readUInt64() throws -> UInt64 {
        try readInteger(UInt64.self)
    }

    private mutating func readInt64() throws -> Int64 {
        try readInteger(Int64.self)
    }

    private mutating func readInteger<T: FixedWidthInteger>(_ type: T.Type) throws -> T {
        let width = MemoryLayout<T>.size
        guard offset <= bytes.count, width <= bytes.count - offset else {
            throw GGUFMetadataError.truncated
        }
        let value = bytes.loadUnaligned(fromByteOffset: offset, as: T.self)
        offset += width
        return T(littleEndian: value)
    }
}

private enum GGUFValueType: UInt32 {
    case uint8 = 0
    case int8 = 1
    case uint16 = 2
    case int16 = 3
    case uint32 = 4
    case int32 = 5
    case float32 = 6
    case bool = 7
    case string = 8
    case array = 9
    case uint64 = 10
    case int64 = 11
    case float64 = 12

    var fixedWidth: Int? {
        switch self {
        case .uint8, .int8, .bool: 1
        case .uint16, .int16: 2
        case .uint32, .int32, .float32: 4
        case .uint64, .int64, .float64: 8
        case .string, .array: nil
        }
    }
}
