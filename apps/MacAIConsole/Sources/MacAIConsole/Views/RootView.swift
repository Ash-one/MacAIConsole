import SwiftUI

enum Page: Hashable {
    case runtime
    case tasks
    case models
    case logs
    case settings
}

struct RootView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router

    var body: some View {
        @Bindable var router = router
        return NavigationSplitView {
            VStack(spacing: 0) {
                sidebarHeader
                List(selection: $router.page) {
                    Label("运行状态", systemImage: "gauge")
                        .tag(Page.runtime)
                    Label("任务", systemImage: "list.bullet.rectangle.portrait")
                        .tag(Page.tasks)
                    Label("管理", systemImage: "shippingbox")
                        .tag(Page.models)
                    Label("日志", systemImage: "doc.text.magnifyingglass")
                        .tag(Page.logs)
                    Label("设置", systemImage: "gearshape")
                        .tag(Page.settings)
                }
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 280)
            }
        } detail: {
            Group {
                switch router.page {
                case .runtime: RuntimeStatusView()
                case .tasks: TasksView()
                case .models: ModelsView()
                case .logs: LogsView()
                case .settings: SettingsView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background {
                Theme.background.ignoresSafeArea()
            }
            .infoTipPresenter()
            .toolbarBackground(Theme.background, for: .windowToolbar)
        }
        // 设置合并进主窗口后，用主窗口级的 Cmd+, 维持 macOS 设置快捷键惯例。
        .background {
            Button("设置") { router.page = .settings }
                .keyboardShortcut(",")
                .opacity(0)
                .frame(width: 0, height: 0)
                .accessibilityHidden(true)
        }
        .task { controller.bootstrapIfNeeded() }
    }

    /// 侧边栏顶部品牌区：渐变标 + 应用名（两行）+ 守护进程相位点。
    private var sidebarHeader: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(Theme.accentGradient)
                .frame(width: 28, height: 28)
                .overlay {
                    Image(systemName: "cpu.fill")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(.black.opacity(0.8))
                }
            VStack(alignment: .leading, spacing: 1) {
                Text("MacAI")
                    .font(.headline)
                    .lineLimit(1)
                Text("Console")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 10)
            StatusDot(
                color: phaseColor,
                pulse: controller.phase != .offline
            )
            .help(phaseLabel)
            .accessibilityLabel("守护进程\(phaseLabel)")
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
        .padding(.bottom, 10)
    }

    private var phaseColor: Color {
        switch controller.phase {
        case .online: Theme.success
        case .starting, .stopping: Theme.warning
        case .offline: Theme.danger
        }
    }

    private var phaseLabel: String {
        switch controller.phase {
        case .online: "在线"
        case .starting: "启动中"
        case .stopping: "停止中"
        case .offline: "离线"
        }
    }
}
