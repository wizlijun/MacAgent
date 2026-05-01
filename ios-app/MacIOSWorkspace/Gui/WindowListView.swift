import SwiftUI

struct WindowListView: View {
    @Bindable var store: SupervisionStore

    var body: some View {
        List {
            Section("当前监管") {
                if store.entries.isEmpty {
                    Text("无").foregroundStyle(.secondary)
                } else {
                    ForEach(store.entries, id: \.supId) { entry in
                        NavigationLink {
                            GuiStreamDetailView(store: store, entry: entry)
                        } label: {
                            VStack(alignment: .leading) {
                                Text(entry.appName).font(.headline)
                                Text(entry.title).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                    }
                }
            }

            Section("可监管窗口") {
                if store.windows.isEmpty {
                    Text("点 ↻ 刷新").foregroundStyle(.secondary)
                } else {
                    ForEach(store.windows, id: \.windowId) { w in
                        Button {
                            Task {
                                await store.supervise(
                                    windowId: w.windowId,
                                    viewport: Viewport(w: 393, h: 852)
                                )
                            }
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(w.appName).font(.headline)
                                Text(w.title).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("桌面窗口")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    Task { await store.refreshWindows() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
            }
        }
        .task { await store.refreshWindows() }
        .alert(item: rejectBinding()) { reject in
            Alert(
                title: Text("监管失败"),
                message: Text(ErrorMessage.describe(code: reject.code, message: reject.reason)),
                dismissButton: .default(Text("OK")) { store.clearReject() }
            )
        }
    }

    private func rejectBinding() -> Binding<SuperviseRejectInfoIdentifiable?> {
        Binding(
            get: { store.lastReject.map { SuperviseRejectInfoIdentifiable(info: $0) } },
            set: { _ in store.clearReject() }
        )
    }
}

private struct SuperviseRejectInfoIdentifiable: Identifiable {
    let id = UUID()
    let info: SuperviseRejectInfo
    var code: String { info.code }
    var reason: String { info.reason }
}
