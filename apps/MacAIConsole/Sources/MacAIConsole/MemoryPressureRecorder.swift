import Foundation

/// 单个系统内存压力采样点：一次成功 `/api/runtime` 读数的占用率快照。
struct MemoryPressureSample: Equatable, Identifiable {
    let timestamp: Date
    /// 系统物理内存占用率（used/total），钳制在 0...1。
    let usedFraction: Double

    var id: Date { timestamp }
}

/// 系统内存压力采样环形缓冲。
///
/// 采样挂在 DaemonController 在线轮询的成功路径上（全应用唯一节拍源）；
/// 缺失读数（daemon 离线、内存字段缺席）不出样本，缺口由时间戳如实呈现，
/// 不插值、不回填。缓冲不随 daemon 重启清空：占用率是宿主机事实，
/// 跨 daemon 生命周期的记录依旧真实。
struct MemoryPressureRecorder {
    /// 缓冲容量：2s 在线节拍下 5 分钟显示窗口约 150 点，2 倍余量容忍节拍退化。
    static let capacity = 300
    /// 显示窗口长度（秒）。
    static let window: TimeInterval = 300

    private(set) var samples: [MemoryPressureSample] = []

    private let clock: () -> Date

    /// - Parameter clock: 采样时刻来源；生产用系统时间，单测注入受控时钟。
    init(clock: @escaping () -> Date = Date.init) {
        self.clock = clock
    }

    /// 记录一次内存读数；total 缺席或 ≤ 0、used 缺席时不采样。
    mutating func record(usedBytes: UInt64?, totalBytes: UInt64?) {
        guard let totalBytes, totalBytes > 0, let usedBytes else { return }
        samples.append(MemoryPressureSample(
            timestamp: clock(),
            usedFraction: min(Double(usedBytes) / Double(totalBytes), 1)
        ))
        if samples.count > Self.capacity {
            samples.removeFirst(samples.count - Self.capacity)
        }
    }

    /// 相对最新样本回看 `window` 秒的显示窗口切片。
    /// 以最新样本而非墙钟为基准：daemon 离线后曲线停在末样本处，而不是被逐渐清空。
    static func displayWindowSamples(
        of samples: [MemoryPressureSample],
        window: TimeInterval = MemoryPressureRecorder.window
    ) -> [MemoryPressureSample] {
        guard let latest = samples.last?.timestamp else { return [] }
        let cutoff = latest.addingTimeInterval(-window)
        return samples.filter { $0.timestamp >= cutoff }
    }
}
