import SwiftUI

struct SessionListView: View {
    @Bindable var store: SessionStore

    var body: some View {
        List {
            Section("启动新会话") {
                ForEach(SessionStore.defaultLaunchers, id: \.id) { item in
                    Button(item.label) {
                        Task { await store.launch(launcherId: item.id) }
                    }
                }
            }
            Section("活动会话") {
                if store.sessions.isEmpty {
                    Text("无活动会话").foregroundStyle(.secondary)
                } else {
                    ForEach(store.sessions) { session in
                        NavigationLink(destination: SessionDetailView(store: store, sid: session.sid)) {
                            VStack(alignment: .leading) {
                                Text(session.label).font(.headline)
                                Text("pid=\(session.pid) · \(session.cols)×\(session.rows) · \(session.streaming ? "streaming" : "idle")")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("会话")
    }
}

struct SessionDetailView: View {
    @Bindable var store: SessionStore
    let sid: String
    @State private var inputText: String = ""

    var body: some View {
        VStack(spacing: 0) {
            if let state = store.sessionStates[sid] {
                TermView(
                    lines: state.lines,
                    cursorRow: state.cursorRow,
                    cursorCol: state.cursorCol,
                    cursorVisible: state.cursorVisible,
                    cols: state.cols,
                    rows: state.rows
                )
            } else {
                Text("等待 snapshot...")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color.black.opacity(0.92))
            }
            Divider()
            InputBar(
                text: $inputText,
                onSendText: { text in
                    Task { await store.sendInput(sid: sid, text: text) }
                },
                onKey: { key in
                    Task { await store.sendKey(sid: sid, key: key) }
                }
            )
        }
        .navigationTitle(sessionLabel)
        .task {
            await store.attach(sid: sid)
        }
        .onDisappear {
            Task { await store.detach(sid: sid) }
        }
    }

    private var sessionLabel: String {
        store.sessions.first(where: { $0.sid == sid })?.label ?? sid
    }
}
