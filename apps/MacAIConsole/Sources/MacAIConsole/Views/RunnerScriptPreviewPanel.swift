import SwiftUI

struct RunnerScriptPreviewPanel: View {
    let preview: RunnerScriptPreview

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            LabeledContent("Runner", value: "\(preview.id) · \(preview.version)")
            LabeledContent("能力", value: preview.capability)
            LabeledContent("适配器", value: preview.adapter)
            LabeledContent("Python", value: preview.requiresPython)
            LabeledContent("依赖", value: preview.dependencies.isEmpty ? "无" : preview.dependencies.joined(separator: ", "))
            LabeledContent("运行期网络", value: preview.networkDuringRuntime ? "已声明" : "关闭")
            LabeledContent("目录检测器", value: preview.localDetectorIDs.isEmpty ? "无" : preview.localDetectorIDs.joined(separator: ", "))
            LabeledContent("源码摘要", value: preview.sourceDigest)
        }
        .font(.footnote)
        .textSelection(.enabled)
        .padding(12)
        .background(.quaternary, in: .rect(cornerRadius: Theme.Radius.control))
    }
}
