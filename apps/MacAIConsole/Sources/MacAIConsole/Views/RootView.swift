import SwiftUI

enum Page: Hashable {
    case runtime
    case tasks
    case models
    case logs
}

struct RootView: View {
    @Environment(DaemonController.self) private var controller
    @State private var page: Page = .runtime

    var body: some View {
        NavigationSplitView {
            VStack(spacing: 0) {
                sidebarHeader
                List(selection: $page) {
                    Label("运行状态", systemImage: "gauge")
                        .tag(Page.runtime)
                    Label("任务", systemImage: "list.bullet.rectangle.portrait")
                        .tag(Page.tasks)
                    Label("模型管理", systemImage: "shippingbox")
                        .tag(Page.models)
                    Label("日志", systemImage: "doc.text.magnifyingglass")
                        .tag(Page.logs)
                }
                .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 240)
            }
        } detail: {
            Group {
                switch page {
                case .runtime: RuntimeStatusView()
                case .tasks: TasksView()
                case .models: ModelsView()
                case .logs: LogsView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background {
                Theme.background.ignoresSafeArea()
            }
            .toolbarBackground(Theme.background, for: .windowToolbar)
        }
        .task { controller.bootstrapIfNeeded() }
    }

    /// 侧边栏顶部品牌区：渐变标 + 应用名 + 守护进程相位点。
    private var sidebarHeader: some View {
        HStack(spacing: 10) {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(Theme.accentGradient)
                .frame(width: 24, height: 24)
                .overlay {
                    Image(systemName: "cpu.fill")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(.black.opacity(0.8))
                }
            Text("MacAI Console")
                .font(.headline)
            Spacer(minLength: 8)
            StatusDot(
                color: phaseColor,
                pulse: controller.phase != .offline
            )
            .help(phaseLabel)
            .accessibilityLabel("守护进程\(phaseLabel)")
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 6)
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
