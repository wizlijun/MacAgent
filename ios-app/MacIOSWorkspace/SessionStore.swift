import Foundation
import Observation

// MARK: - PendingLaunch

struct PendingLaunch: Identifiable {
    let id: String          // reqId
    let launcherId: String
    var status: Status
    var settledAt: Date?    // set when succeeded or rejected (for pruning)

    enum Status { case running, succeeded(sid: String), rejected(code: String, reason: String) }
}

@MainActor
@Observable
final class SessionStore {
    private(set) var sessions: [SessionInfo] = []
    private(set) var sessionStates: [String: TermSessionState] = [:]
    private(set) var pendingLaunches: [String: PendingLaunch] = [:]
    private(set) var pendingLaunchVersion: Int = 0   // bumped on every mutation
    private(set) var connectedTick: Int = 0

    var clipboardStore: ClipboardStore?   // PairedView 注入
    var watcherStore: WatcherStore?      // PairedView 注入
    var supervisionStore: SupervisionStore?  // PairedView 注入

    private let glue: RtcGlue?

    init(glue: RtcGlue?) {
        self.glue = glue
    }

    func bind() async {
        guard let glue else { return }
        for await payload in await glue.ctrlMessages() {
            handle(payload)
        }
    }

    func onGlueConnected() async {
        connectedTick &+= 1
        guard let glue else { return }
        for s in sessions {
            await glue.sendCtrl(.attachSession(sid: s.sid))
        }
    }

    private func handle(_ payload: CtrlPayload) {
        switch payload {
        case .sessionList(let list):
            sessions = list
            sessionStates = sessionStates.filter { id, _ in list.contains(where: { $0.sid == id }) }

        case .sessionAdded(let info):
            if !sessions.contains(where: { $0.sid == info.sid }) {
                sessions.append(info)
            }

        case .sessionRemoved(let sid, _):
            sessions.removeAll(where: { $0.sid == sid })
            sessionStates.removeValue(forKey: sid)

        case .sessionExited(let sid, _, _):
            if let i = sessions.firstIndex(where: { $0.sid == sid }) {
                let s = sessions[i]
                sessions[i] = SessionInfo(
                    sid: s.sid, label: s.label, argv: s.argv, pid: s.pid,
                    cols: s.cols, rows: s.rows, startedTs: s.startedTs,
                    streaming: false, source: s.source
                )
            }

        case .termSnapshot(let sid, let revision, let cols, let rows,
                           let cr, let cc, let cv, let title, let lines):
            sessionStates[sid] = TermSessionState(
                revision: revision, cols: cols, rows: rows,
                cursorRow: cr, cursorCol: cc, cursorVisible: cv,
                title: title, lines: lines,
                history: sessionStates[sid]?.history ?? []
            )

        case .termDelta(let sid, let revision, let cols, let rows,
                        let cr, let cc, let cv, let title, let lines):
            guard var st = sessionStates[sid] else { return }
            st.revision = revision; st.cols = cols; st.rows = rows
            st.cursorRow = cr; st.cursorCol = cc; st.cursorVisible = cv
            st.title = title
            for line in lines {
                let i = Int(line.index)
                if i < st.lines.count {
                    st.lines[i] = line
                } else {
                    while st.lines.count <= i {
                        st.lines.append(TerminalLine(
                            index: UInt16(st.lines.count), runs: [], wrapped: false
                        ))
                    }
                    st.lines[i] = line
                }
            }
            if st.lines.count > Int(rows) {
                st.lines = Array(st.lines.prefix(Int(rows)))
            }
            sessionStates[sid] = st

        case .termHistorySnapshot(let sid, _, let lines):
            if var st = sessionStates[sid] {
                st.history = lines
                sessionStates[sid] = st
            }

        case .termHistoryAppend(let sid, _, let lines):
            if var st = sessionStates[sid] {
                st.history.append(contentsOf: lines)
                if st.history.count > 1000 {
                    st.history.removeFirst(st.history.count - 1000)
                }
                sessionStates[sid] = st
            }

        case .launchAck(let reqId, let sid):
            pendingLaunches[reqId]?.status = .succeeded(sid: sid)
            pendingLaunches[reqId]?.settledAt = Date()
            pendingLaunchVersion &+= 1
            schedulePrune(reqId: reqId)

        case .launchReject(let reqId, let code, let reason):
            pendingLaunches[reqId]?.status = .rejected(code: code, reason: reason)
            pendingLaunches[reqId]?.settledAt = Date()
            pendingLaunchVersion &+= 1

        case .clipboardSet(let source, let content):
            if case .mac = source {  // 只接 Mac → iOS 方向
                clipboardStore?.handleRemote(content)
            }

        case .watchersList(let sid, let watchers):
            watcherStore?.handleList(sid: sid, list: watchers)

        case .watcherMatched(let sid, let watcherId, let lineText):
            watcherStore?.handleMatched(sid: sid, watcherId: watcherId, lineText: lineText)

        case .windowsList(let windows):
            supervisionStore?.handleWindowsList(windows)

        case .supervisedAck(let supId, let entry):
            supervisionStore?.handleSupervisedAck(supId: supId, entry: entry)

        case .superviseReject(let windowId, let code, let reason):
            supervisionStore?.handleSuperviseReject(windowId: windowId, code: code, reason: reason)

        case .streamEnded(let supId, let reason):
            supervisionStore?.handleStreamEnded(supId: supId, reason: reason)

        case .supervisionList(let entries):
            supervisionStore?.handleSupervisionList(entries)

        default:
            break
        }
    }

    func clearPendingLaunch(reqId: String) {
        pendingLaunches.removeValue(forKey: reqId)
        pendingLaunchVersion &+= 1
    }

    private func schedulePrune(reqId: String) {
        Task {
            try? await Task.sleep(nanoseconds: 10_000_000_000) // 10 s
            pendingLaunches.removeValue(forKey: reqId)
            pendingLaunchVersion &+= 1
        }
    }

    // MARK: - Outbound

    @discardableResult
    func launch(launcherId: String) async -> String {
        let reqId = UUID().uuidString
        pendingLaunches[reqId] = PendingLaunch(id: reqId, launcherId: launcherId, status: .running, settledAt: nil)
        pendingLaunchVersion &+= 1
        await glue?.sendCtrl(.launchSession(reqId: reqId, launcherId: launcherId, cwdOverride: nil))
        return reqId
    }

    func attach(sid: String) async {
        await glue?.sendCtrl(.attachSession(sid: sid))
    }

    func detach(sid: String) async {
        await glue?.sendCtrl(.detachSession(sid: sid))
    }

    func kill(sid: String) async {
        await glue?.sendCtrl(.killSession(sid: sid))
    }

    func sendInput(sid: String, text: String) async {
        await glue?.sendCtrl(.input(sid: sid, payload: .text(data: text)))
    }

    func sendKey(sid: String, key: InputKey) async {
        await glue?.sendCtrl(.input(sid: sid, payload: .key(key: key)))
    }

    func resize(sid: String, cols: UInt16, rows: UInt16) async {
        await glue?.sendCtrl(.resize(sid: sid, cols: cols, rows: rows))
    }
}

// MARK: - TermSessionState

struct TermSessionState: Equatable {
    var revision: UInt64
    var cols: UInt16
    var rows: UInt16
    var cursorRow: UInt16
    var cursorCol: UInt16
    var cursorVisible: Bool
    var title: String?
    var lines: [TerminalLine]
    var history: [String]
}

// MARK: - Default launchers

extension SessionStore {
    static let defaultLaunchers: [(id: String, label: String)] = [
        ("zsh",         "Zsh shell"),
        ("claude-code", "Claude Code"),
        ("codex",       "Codex"),
        ("npm-test",    "npm test"),
        ("git-status",  "git status"),
    ]
}
