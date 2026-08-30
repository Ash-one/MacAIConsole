import Foundation
import Observation

/// 主窗口侧边栏导航的共享状态，供页面内跳转（如运行状态页的内存预算修改）复用。
@MainActor
@Observable
final class AppRouter {
    var page: Page = .runtime

    init() {}
}
