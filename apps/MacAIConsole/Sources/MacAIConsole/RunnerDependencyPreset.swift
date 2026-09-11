import Foundation

struct RunnerDependencyPreset: Decodable, Identifiable, Hashable {
    var id: String
    var title: String
    var requirements: [String]
}
