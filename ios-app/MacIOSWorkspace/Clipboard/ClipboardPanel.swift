import SwiftUI

struct ClipboardPanel: View {
    @Bindable var store: ClipboardStore
    @State private var sendText: String = ""

    var body: some View {
        Form {
            Section("发送到 Mac") {
                TextField("输入要复制到 Mac 剪贴板的文本", text: $sendText, axis: .vertical)
                    .lineLimit(1...10)
                    .autocorrectionDisabled()
                HStack {
                    Button("取最近 iOS 复制") {
                        sendText = UIPasteboard.general.string ?? ""
                    }
                    .buttonStyle(.bordered)
                    Spacer()
                    Button("发送") {
                        let text = sendText
                        sendText = ""
                        Task { await store.sendToMac(text) }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(sendText.isEmpty)
                }
            }

            Section("最近从 Mac 收到") {
                if store.history.isEmpty {
                    Text("暂无").foregroundStyle(.secondary)
                } else {
                    ForEach(store.history) { entry in
                        Button {
                            UIPasteboard.general.string = entry.text
                        } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(entry.text)
                                    .font(.system(.body, design: .monospaced))
                                    .lineLimit(3)
                                    .foregroundStyle(.primary)
                                Text(entry.timestamp.formatted(.relative(presentation: .named)))
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("剪贴板")
    }
}
