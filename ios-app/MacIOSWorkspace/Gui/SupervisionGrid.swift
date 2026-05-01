import SwiftUI

/// Grid of supervised windows with add tile + count header.
struct SupervisionGrid: View {
    @Bindable var store: SupervisionStore
    @Environment(\.horizontalSizeClass) private var hSizeClass
    @State private var showingAdd = false
    @State private var showingLaunch = false
    @State private var showingPickWindow = false

    private var columnCount: Int { hSizeClass == .compact ? 2 : 3 }

    private var columns: [GridItem] {
        Array(repeating: GridItem(.flexible(), spacing: 12), count: columnCount)
    }

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 12) {
                ForEach(store.entries, id: \.supId) { entry in
                    NavigationLink {
                        GuiStreamDetailView(store: store, entry: entry)
                    } label: {
                        SupervisionTile(entry: entry, store: store)
                    }
                    .buttonStyle(.plain)
                    .simultaneousGesture(TapGesture().onEnded {
                        if entry.status != .active {
                            store.requestSwitchActive(supId: entry.supId)
                        }
                    })
                }
                if store.entries.count < 8 {
                    Button { showingAdd = true } label: { addTile }
                        .buttonStyle(.plain)
                }
            }
            .padding(12)
        }
        .navigationTitle("\(store.entries.count) / 8 监管中")
        .confirmationDialog("添加监管", isPresented: $showingAdd) {
            Button("监管现有窗口") { showingPickWindow = true }
            Button("启动 App") { showingLaunch = true }
            Button("取消", role: .cancel) { }
        }
        .sheet(isPresented: $showingLaunch) {
            LaunchAppSheet(store: store)
        }
        .sheet(isPresented: $showingPickWindow) {
            NavigationStack { WindowListView(store: store) }
        }
    }

    private var addTile: some View {
        VStack {
            Image(systemName: "plus.rectangle.on.rectangle")
                .font(.system(size: 40))
                .foregroundStyle(.secondary)
            Text("添加").font(.caption)
        }
        .frame(maxWidth: .infinity)
        .aspectRatio(4.0 / 3.0, contentMode: .fit)
        .background(Color.gray.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
