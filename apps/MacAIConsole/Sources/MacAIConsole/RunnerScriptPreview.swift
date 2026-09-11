import Foundation

struct RunnerScriptPreview: Decodable, Hashable {
    var id: String
    var version: String
    var capability: String
    var adapter: String
    var modelFormat: String
    var requiresPython: String
    var dependencies: [String]
    var networkDuringRuntime: Bool
    var sourceDigest: String
    var localDetectorIDs: [String]

    enum CodingKeys: String, CodingKey {
        case id, version, capability, adapter, dependencies
        case modelFormat = "model_format"
        case requiresPython = "requires_python"
        case networkDuringRuntime = "network_during_runtime"
        case sourceDigest = "source_digest"
        case localDetectorIDs = "local_detector_ids"
    }
}
