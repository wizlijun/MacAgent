import SwiftUI

struct SessionListView: View {
    @Bindable var store: SessionStore
    @State private var presentedSid: String?
    @State private var rejectedReqId: String?

    var body: some View {
        List {
            Section("启动新会话") {
                ForEach(SessionStore.defaultLaunchers, id: \.id) { item in
                    LauncherRow(store: store, item: item)
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
        .navigationDestination(item: $presentedSid) { sid in
            SessionDetailView(store: store, sid: sid)
        }
        .onChange(of: store.pendingLaunchVersion) { _, _ in
            for launch in store.pendingLaunches.values {
                if case .succeeded(let sid) = launch.status, presentedSid != sid {
                    presentedSid = sid
                    return
                }
                if case .rejected = launch.status, rejectedReqId == nil {
                    rejectedReqId = launch.id
                }
            }
        }
        .alert("启动失败", isPresented: rejectAlertBinding) {
            Button("OK") {
                if let rid = rejectedReqId {
                    store.clearPendingLaunch(reqId: rid)
                    rejectedReqId = nil
                }
            }
        } message: {
            if let rid = rejectedReqId, let p = store.pendingLaunches[rid],
               case .rejected(let code, let reason) = p.status {
                Text(ErrorMessage.describe(code: code, message: reason))
            }
        }
    }

    private var rejectAlertBinding: Binding<Bool> {
        Binding(
            get: { rejectedReqId != nil },
            set: { if !$0 { rejectedReqId = nil } }
        )
    }
}

private struct LauncherRow: View {
    @Bindable var store: SessionStore
    let item: (id: String, label: String)

    private var isRunning: Bool {
        store.pendingLaunches.values.contains {
            $0.launcherId == item.id && { if case .running = $0.status { return true } else { return false } }($0)
        }
    }

    var body: some View {
        Button(action: { Task { _ = await store.launch(launcherId: item.id) } }) {
            HStack {
                Text(item.label)
                Spacer()
                if isRunning {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .disabled(isRunning)
    }
}

struct SessionDetailView: View {
    @Bindable var store: SessionStore
    let sid: String
    @State private var inputText: String = ""
    @State private var composeText: String = ""        // M4.4 新增
    @State private var presentingCompose: Bool = false  // M4.4 新增

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
                },
                onCompose: { presentingCompose = true }   // M4.4 新增
            )
        }
        .navigationTitle(sessionLabel)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if let ws = store.watcherStore {
                    NavigationLink {
                        WatchersView(store: ws, sid: sid)
                    } label: {
                        Image(systemName: "bell.badge")
                    }
                }
            }
        }
        .task {
            await store.attach(sid: sid)
        }
        .onChange(of: store.connectedTick) { _, _ in
            Task { await store.attach(sid: sid) }
        }
        .onDisappear {
            Task { await store.detach(sid: sid) }
        }
        .sheet(isPresented: $presentingCompose) {   // M4.4 新增
            ComposeSheet(
                text: $composeText,
                title: "Compose · \(sessionLabel)",
                onSend: { sent in
                    Task { await store.sendInput(sid: sid, text: sent) }
                },
                onCancel: {}
            )
        }
    }

    private var sessionLabel: String {
        store.sessions.first(where: { $0.sid == sid })?.label ?? sid
    }
}
