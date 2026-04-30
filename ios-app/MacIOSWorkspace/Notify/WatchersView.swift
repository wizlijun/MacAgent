import SwiftUI

struct WatchersView: View {
    @Bindable var store: WatcherStore
    let sid: String
    @State private var showAddSheet = false

    var body: some View {
        List {
            Section("正则提醒") {
                let list = store.watchers[sid] ?? []
                if list.isEmpty {
                    Text("暂无").foregroundStyle(.secondary)
                } else {
                    ForEach(list, id: \.id) { w in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(w.name).font(.headline)
                                Spacer()
                                Text("\(w.hits) 次").font(.caption).foregroundStyle(.secondary)
                            }
                            Text(w.regex).font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.secondary)
                            if let last = w.last_match {
                                Text("最近: \(last)").font(.caption).foregroundStyle(.tertiary)
                                    .lineLimit(1)
                            }
                        }
                        .swipeActions {
                            Button(role: .destructive) {
                                Task { await store.remove(sid: sid, watcherId: w.id) }
                            } label: { Label("删除", systemImage: "trash") }
                        }
                    }
                }
            }

            Section("最近命中") {
                let recent = store.matches[sid] ?? []
                if recent.isEmpty {
                    Text("暂无").foregroundStyle(.secondary)
                } else {
                    ForEach(recent) { m in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(m.lineText).font(.system(.caption, design: .monospaced))
                                .lineLimit(2)
                            Text(m.timestamp.formatted(.relative(presentation: .named)))
                                .font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
        .navigationTitle("正则提醒")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { showAddSheet = true } label: {
                    Image(systemName: "plus")
                }
            }
        }
        .sheet(isPresented: $showAddSheet) {
            AddWatcherSheet(store: store, sid: sid)
        }
    }
}

struct AddWatcherSheet: View {
    @Bindable var store: WatcherStore
    let sid: String
    @State private var name: String = ""
    @State private var regex: String = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Name") {
                    TextField("e.g. errors", text: $name)
                }
                Section("Regex") {
                    TextField("e.g. error.*", text: $regex)
                        .font(.system(.body, design: .monospaced))
                        .autocorrectionDisabled()
                }
            }
            .navigationTitle("Add watcher")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Add") {
                        let n = name.isEmpty ? "watcher" : name
                        let r = regex
                        Task {
                            await store.add(sid: sid, regex: r, name: n)
                            await MainActor.run { dismiss() }
                        }
                    }
                    .disabled(regex.isEmpty)
                }
            }
        }
    }
}
