import SwiftUI

enum Page: Hashable {
    case runtime
    case models
}

struct RootView: View {
    @Environment(DaemonController.self) private var controller
    @State private var page: Page = .runtime

    var body: some View {
        NavigationSplitView {
            List(selection: $page) {
                Label("运行状态", systemImage: "gauge.with.dotted.needle")
                    .tag(Page.runtime)
                Label("模型管理", systemImage: "shippingbox")
                    .tag(Page.models)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 240)
        } detail: {
            switch page {
            case .runtime: RuntimeStatusView()
            case .models: ModelsView()
            }
        }
        .task { controller.bootstrapIfNeeded() }
    }
}