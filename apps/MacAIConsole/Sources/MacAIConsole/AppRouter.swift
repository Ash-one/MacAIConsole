import Foundation
import Observation

/// 主窗口侧边栏导航的共享状态，供页面内跳转（如运行状态页的内存预算修改）复用。
@MainActor
@Observable
final class AppRouter {
    var page: Page = .runtime

    init() {}

    /// 页面内引导跳转：如模型页「Provider 环境未就绪」引导用户去设置安装引擎。
    func goToSettings() {
        page = .settings
    }
}
