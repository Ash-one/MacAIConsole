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
        } detail: {
            switch page {
            case .runtime: RuntimeStatusView()
            case .tasks: TasksView()
            case .models: ModelsView()
            case .logs: LogsView()
            }
        }
        .task { controller.bootstrapIfNeeded() }
    }
}