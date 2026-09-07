import Foundation
import Observation

/// 主窗口侧边栏导航的共享状态，供页面内跳转（如运行状态页的内存预算修改）复用。
@MainActor
@Observable
final class AppRouter {
    var page: Page = .runtime

    init() {}

    /// 页面内引导跳转：如运行状态页内存预算卡片「修改」跳转设置页。
    func goToSettings() {
        page = .settings
    }

    /// 页面内引导跳转：导航至「管理」页（包含顶端引擎状态与模型管理）。
    func goToManagement() {
        page = .models
    }
}
