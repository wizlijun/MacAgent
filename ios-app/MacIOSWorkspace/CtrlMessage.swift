import Foundation

// ---------------------------------------------------------------------------
// Shared terminal types
// ---------------------------------------------------------------------------

struct TerminalLine: Codable, Equatable {
    let index: UInt16
    let runs: [TerminalRun]
    let wrapped: Bool
}

struct TerminalRun: Codable, Equatable {
    let text: String
    let fg: TerminalColor?
    let bg: TerminalColor?
    let bold: Bool
    let dim: Bool
    let italic: Bool
    let underline: Bool
    let inverse: Bool
}

enum TerminalColor: Codable, Equatable {
    case indexed(value: UInt8)
    case rgb(r: UInt8, g: UInt8, b: UInt8)

    private enum Kind: String, Codable { case indexed, rgb }
    private enum CodingKeys: String, CodingKey { case kind, value, r, g, b }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(Kind.self, forKey: .kind)
        switch kind {
        case .indexed:
            self = .indexed(value: try c.decode(UInt8.self, forKey: .value))
        case .rgb:
            self = .rgb(
                r: try c.decode(UInt8.self, forKey: .r),
                g: try c.decode(UInt8.self, forKey: .g),
                b: try c.decode(UInt8.self, forKey: .b)
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .indexed(let value):
            try c.encode(Kind.indexed, forKey: .kind)
            try c.encode(value, forKey: .value)
        case .rgb(let r, let g, let b):
            try c.encode(Kind.rgb, forKey: .kind)
            try c.encode(r, forKey: .r)
            try c.encode(g, forKey: .g)
            try c.encode(b, forKey: .b)
        }
    }
}

enum InputKey: String, Codable {
    case enter, tab
    case shiftTab = "shift_tab"
    case backspace, escape
    case arrowUp = "arrow_up"
    case arrowDown = "arrow_down"
    case arrowLeft = "arrow_left"
    case arrowRight = "arrow_right"
    case home, end
    case pageUp = "page_up"
    case pageDown = "page_down"
    case delete
    case ctrlA = "ctrl_a"
    case ctrlC = "ctrl_c"
    case ctrlD = "ctrl_d"
    case ctrlE = "ctrl_e"
    case ctrlK = "ctrl_k"
    case ctrlL = "ctrl_l"
    case ctrlR = "ctrl_r"
    case ctrlU = "ctrl_u"
    case ctrlW = "ctrl_w"
    case ctrlZ = "ctrl_z"
    case f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12
}

enum TerminalInput: Codable, Equatable {
    case text(data: String)
    case key(key: InputKey)

    private enum Kind: String, Codable { case text, key }
    private enum CodingKeys: String, CodingKey { case kind, data, key }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(Kind.self, forKey: .kind)
        switch kind {
        case .text:
            self = .text(data: try c.decode(String.self, forKey: .data))
        case .key:
            self = .key(key: try c.decode(InputKey.self, forKey: .key))
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let data):
            try c.encode(Kind.text, forKey: .kind)
            try c.encode(data, forKey: .data)
        case .key(let key):
            try c.encode(Kind.key, forKey: .kind)
            try c.encode(key, forKey: .key)
        }
    }
}

struct SessionInfo: Codable, Equatable, Identifiable {
    var id: String { sid }
    let sid: String
    let label: String
    let argv: [String]
    let pid: UInt32
    let cols: UInt16
    let rows: UInt16
    let startedTs: UInt64
    let streaming: Bool
    let source: SessionSource

    enum CodingKeys: String, CodingKey {
        case sid, label, argv, pid, cols, rows
        case startedTs = "started_ts"
        case streaming, source
    }
}

enum SessionSource: Codable, Equatable {
    case iosLaunched(launcherId: String)
    case userManual

    private enum Kind: String, Codable {
        case ios_launched
        case user_manual
    }
    private enum CodingKeys: String, CodingKey { case kind, launcher_id }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(Kind.self, forKey: .kind)
        switch kind {
        case .ios_launched:
            self = .iosLaunched(launcherId: try c.decode(String.self, forKey: .launcher_id))
        case .user_manual:
            self = .userManual
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .iosLaunched(let launcherId):
            try c.encode(Kind.ios_launched, forKey: .kind)
            try c.encode(launcherId, forKey: .launcher_id)
        case .userManual:
            try c.encode(Kind.user_manual, forKey: .kind)
        }
    }
}

// ---------------------------------------------------------------------------
// Clipboard types (M4)
// ---------------------------------------------------------------------------

enum ClipSource: Codable, Equatable {
    case mac
    case ios

    private enum CodingKeys: String, CodingKey { case kind }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        switch kind {
        case "mac": self = .mac
        case "ios": self = .ios
        default:
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "unknown ClipSource kind \(kind)"
            ))
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .mac: try c.encode("mac", forKey: .kind)
        case .ios: try c.encode("ios", forKey: .kind)
        }
    }
}

enum ClipContent: Codable, Equatable {
    case text(data: String)

    private enum CodingKeys: String, CodingKey { case kind, data }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        switch kind {
        case "text":
            self = .text(data: try c.decode(String.self, forKey: .data))
        default:
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "unknown ClipContent kind \(kind)"
            ))
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let data):
            try c.encode("text", forKey: .kind)
            try c.encode(data, forKey: .data)
        }
    }
}

// ---------------------------------------------------------------------------
// Watcher types (M4.6)
// ---------------------------------------------------------------------------

struct WatcherInfo: Codable, Equatable {
    let id: String
    let regex: String
    let name: String
    let hits: UInt32
    let last_match: String?
}

// ---------------------------------------------------------------------------
// CtrlPayload
// ---------------------------------------------------------------------------

enum CtrlPayload: Codable, Equatable {
    // M1/M2 existing
    case ping(ts: UInt64, nonce: String)
    case pong(ts: UInt64, nonce: String)
    case heartbeat(ts: UInt64, nonce: String)
    case heartbeatAck(ts: UInt64, nonce: String)
    case error(code: String, msg: String)

    // M3 v2: session management
    case launchSession(reqId: String, launcherId: String, cwdOverride: String?)
    case launchAck(reqId: String, sid: String)
    case launchReject(reqId: String, code: String, reason: String)
    case attachSession(sid: String)
    case detachSession(sid: String)
    case killSession(sid: String)
    case sessionList(sessions: [SessionInfo])
    case sessionAdded(session: SessionInfo)
    case sessionRemoved(sid: String, reason: String)
    case sessionExited(sid: String, exitStatus: Int32?, reason: String)

    // M3 v2: terminal data
    case termSnapshot(sid: String, revision: UInt64, cols: UInt16, rows: UInt16,
                      cursorRow: UInt16, cursorCol: UInt16, cursorVisible: Bool,
                      title: String?, lines: [TerminalLine])
    case termDelta(sid: String, revision: UInt64, cols: UInt16, rows: UInt16,
                   cursorRow: UInt16, cursorCol: UInt16, cursorVisible: Bool,
                   title: String?, lines: [TerminalLine])
    case termHistorySnapshot(sid: String, revision: UInt64, lines: [String])
    case termHistoryAppend(sid: String, revision: UInt64, lines: [String])

    // M3 v2: input
    case input(sid: String, payload: TerminalInput)
    case resize(sid: String, cols: UInt16, rows: UInt16)

    // M4: clipboard
    case clipboardSet(source: ClipSource, content: ClipContent)

    // M4.6: notify watchers
    case watchSession(sid: String, watcherId: String, regex: String, name: String)
    case unwatchSession(sid: String, watcherId: String)
    case watchersList(sid: String, watchers: [WatcherInfo])
    case watcherMatched(sid: String, watcherId: String, lineText: String)

    // MARK: - Coding keys

    private enum CodingKeys: String, CodingKey {
        case type
        case ts, nonce, code, msg
        case req_id, launcher_id, cwd_override, sid, reason
        case exit_status, session, sessions
        case revision, cols, rows, cursor_row, cursor_col, cursor_visible, title, lines
        case payload
        case source, content
        case watcher_id, regex, name, watchers, line_text
    }

    // MARK: - canonical bytes

    func canonicalBytes() throws -> Data {
        switch self {
        case .ping(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "ping", "ts": ts, "nonce": nonce])
        case .pong(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "pong", "ts": ts, "nonce": nonce])
        case .heartbeat(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "heartbeat", "ts": ts, "nonce": nonce])
        case .heartbeatAck(let ts, let nonce):
            return try CanonicalJSON.encode(["type": "heartbeat_ack", "ts": ts, "nonce": nonce])
        case .error(let code, let msg):
            return try CanonicalJSON.encode(["type": "error", "code": code, "msg": msg])

        case .launchSession(let reqId, let launcherId, let cwdOverride):
            var d: [String: Any] = ["type": "launch_session", "req_id": reqId, "launcher_id": launcherId]
            if let cwd = cwdOverride { d["cwd_override"] = cwd } else { d["cwd_override"] = NSNull() }
            return try CanonicalJSON.encode(d)
        case .launchAck(let reqId, let sid):
            return try CanonicalJSON.encode(["type": "launch_ack", "req_id": reqId, "sid": sid])
        case .launchReject(let reqId, let code, let reason):
            return try CanonicalJSON.encode(["type": "launch_reject", "req_id": reqId, "code": code, "reason": reason])
        case .attachSession(let sid):
            return try CanonicalJSON.encode(["type": "attach_session", "sid": sid])
        case .detachSession(let sid):
            return try CanonicalJSON.encode(["type": "detach_session", "sid": sid])
        case .killSession(let sid):
            return try CanonicalJSON.encode(["type": "kill_session", "sid": sid])
        case .sessionList(let sessions):
            let encoder = JSONEncoder()
            let sessionsData = try encoder.encode(sessions)
            let sessionsObj = try JSONSerialization.jsonObject(with: sessionsData)
            return try CanonicalJSON.encode(["type": "session_list", "sessions": sessionsObj])
        case .sessionAdded(let session):
            let encoder = JSONEncoder()
            encoder.keyEncodingStrategy = .convertToSnakeCase
            let sessionData = try encoder.encode(session)
            let sessionObj = try JSONSerialization.jsonObject(with: sessionData)
            return try CanonicalJSON.encode(["type": "session_added", "session": sessionObj])
        case .sessionRemoved(let sid, let reason):
            return try CanonicalJSON.encode(["type": "session_removed", "sid": sid, "reason": reason])
        case .sessionExited(let sid, let exitStatus, let reason):
            var d: [String: Any] = ["type": "session_exited", "sid": sid, "reason": reason]
            if let s = exitStatus { d["exit_status"] = s } else { d["exit_status"] = NSNull() }
            return try CanonicalJSON.encode(d)
        case .termSnapshot(let sid, let revision, let cols, let rows,
                           let cursorRow, let cursorCol, let cursorVisible, let title, let lines):
            let encoder = JSONEncoder()
            let linesData = try encoder.encode(lines)
            let linesObj = try JSONSerialization.jsonObject(with: linesData)
            var d: [String: Any] = [
                "type": "term_snapshot", "sid": sid, "revision": revision,
                "cols": cols, "rows": rows,
                "cursor_row": cursorRow, "cursor_col": cursorCol,
                "cursor_visible": cursorVisible, "lines": linesObj
            ]
            if let t = title { d["title"] = t } else { d["title"] = NSNull() }
            return try CanonicalJSON.encode(d)
        case .termDelta(let sid, let revision, let cols, let rows,
                        let cursorRow, let cursorCol, let cursorVisible, let title, let lines):
            let encoder = JSONEncoder()
            let linesData = try encoder.encode(lines)
            let linesObj = try JSONSerialization.jsonObject(with: linesData)
            var d: [String: Any] = [
                "type": "term_delta", "sid": sid, "revision": revision,
                "cols": cols, "rows": rows,
                "cursor_row": cursorRow, "cursor_col": cursorCol,
                "cursor_visible": cursorVisible, "lines": linesObj
            ]
            if let t = title { d["title"] = t } else { d["title"] = NSNull() }
            return try CanonicalJSON.encode(d)
        case .termHistorySnapshot(let sid, let revision, let lines):
            return try CanonicalJSON.encode(["type": "term_history_snapshot", "sid": sid, "revision": revision, "lines": lines])
        case .termHistoryAppend(let sid, let revision, let lines):
            return try CanonicalJSON.encode(["type": "term_history_append", "sid": sid, "revision": revision, "lines": lines])
        case .input(let sid, let payload):
            let encoder = JSONEncoder()
            let payloadData = try encoder.encode(payload)
            let payloadObj = try JSONSerialization.jsonObject(with: payloadData)
            return try CanonicalJSON.encode(["type": "input", "sid": sid, "payload": payloadObj])
        case .resize(let sid, let cols, let rows):
            return try CanonicalJSON.encode(["type": "resize", "sid": sid, "cols": cols, "rows": rows])
        case .clipboardSet(let source, let content):
            let encoder = JSONEncoder()
            let sourceData = try encoder.encode(source)
            let sourceObj = try JSONSerialization.jsonObject(with: sourceData)
            let contentData = try encoder.encode(content)
            let contentObj = try JSONSerialization.jsonObject(with: contentData)
            return try CanonicalJSON.encode(["type": "clipboard_set", "source": sourceObj, "content": contentObj])
        case .watchSession(let sid, let watcherId, let regex, let name):
            return try CanonicalJSON.encode(["type": "watch_session", "sid": sid,
                                             "watcher_id": watcherId, "regex": regex, "name": name])
        case .unwatchSession(let sid, let watcherId):
            return try CanonicalJSON.encode(["type": "unwatch_session", "sid": sid,
                                             "watcher_id": watcherId])
        case .watchersList(let sid, let watchers):
            let encoder = JSONEncoder()
            let watchersData = try encoder.encode(watchers)
            let watchersObj = try JSONSerialization.jsonObject(with: watchersData)
            return try CanonicalJSON.encode(["type": "watchers_list", "sid": sid, "watchers": watchersObj])
        case .watcherMatched(let sid, let watcherId, let lineText):
            return try CanonicalJSON.encode(["type": "watcher_matched", "sid": sid,
                                             "watcher_id": watcherId, "line_text": lineText])
        }
    }

    // MARK: - encode(to:)

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .ping(let ts, let nonce):
            try c.encode("ping", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .pong(let ts, let nonce):
            try c.encode("pong", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .heartbeat(let ts, let nonce):
            try c.encode("heartbeat", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .heartbeatAck(let ts, let nonce):
            try c.encode("heartbeat_ack", forKey: .type)
            try c.encode(ts, forKey: .ts); try c.encode(nonce, forKey: .nonce)
        case .error(let code, let msg):
            try c.encode("error", forKey: .type)
            try c.encode(code, forKey: .code); try c.encode(msg, forKey: .msg)

        case .launchSession(let reqId, let launcherId, let cwdOverride):
            try c.encode("launch_session", forKey: .type)
            try c.encode(reqId, forKey: .req_id)
            try c.encode(launcherId, forKey: .launcher_id)
            try c.encode(cwdOverride, forKey: .cwd_override)
        case .launchAck(let reqId, let sid):
            try c.encode("launch_ack", forKey: .type)
            try c.encode(reqId, forKey: .req_id); try c.encode(sid, forKey: .sid)
        case .launchReject(let reqId, let code, let reason):
            try c.encode("launch_reject", forKey: .type)
            try c.encode(reqId, forKey: .req_id)
            try c.encode(code, forKey: .code); try c.encode(reason, forKey: .reason)
        case .attachSession(let sid):
            try c.encode("attach_session", forKey: .type); try c.encode(sid, forKey: .sid)
        case .detachSession(let sid):
            try c.encode("detach_session", forKey: .type); try c.encode(sid, forKey: .sid)
        case .killSession(let sid):
            try c.encode("kill_session", forKey: .type); try c.encode(sid, forKey: .sid)
        case .sessionList(let sessions):
            try c.encode("session_list", forKey: .type); try c.encode(sessions, forKey: .sessions)
        case .sessionAdded(let session):
            try c.encode("session_added", forKey: .type); try c.encode(session, forKey: .session)
        case .sessionRemoved(let sid, let reason):
            try c.encode("session_removed", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(reason, forKey: .reason)
        case .sessionExited(let sid, let exitStatus, let reason):
            try c.encode("session_exited", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(exitStatus, forKey: .exit_status)
            try c.encode(reason, forKey: .reason)

        case .termSnapshot(let sid, let revision, let cols, let rows,
                           let cursorRow, let cursorCol, let cursorVisible, let title, let lines):
            try c.encode("term_snapshot", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(revision, forKey: .revision)
            try c.encode(cols, forKey: .cols); try c.encode(rows, forKey: .rows)
            try c.encode(cursorRow, forKey: .cursor_row); try c.encode(cursorCol, forKey: .cursor_col)
            try c.encode(cursorVisible, forKey: .cursor_visible)
            try c.encode(title, forKey: .title); try c.encode(lines, forKey: .lines)
        case .termDelta(let sid, let revision, let cols, let rows,
                        let cursorRow, let cursorCol, let cursorVisible, let title, let lines):
            try c.encode("term_delta", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(revision, forKey: .revision)
            try c.encode(cols, forKey: .cols); try c.encode(rows, forKey: .rows)
            try c.encode(cursorRow, forKey: .cursor_row); try c.encode(cursorCol, forKey: .cursor_col)
            try c.encode(cursorVisible, forKey: .cursor_visible)
            try c.encode(title, forKey: .title); try c.encode(lines, forKey: .lines)
        case .termHistorySnapshot(let sid, let revision, let lines):
            try c.encode("term_history_snapshot", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(revision, forKey: .revision)
            try c.encode(lines, forKey: .lines)
        case .termHistoryAppend(let sid, let revision, let lines):
            try c.encode("term_history_append", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(revision, forKey: .revision)
            try c.encode(lines, forKey: .lines)

        case .input(let sid, let payload):
            try c.encode("input", forKey: .type)
            try c.encode(sid, forKey: .sid); try c.encode(payload, forKey: .payload)
        case .resize(let sid, let cols, let rows):
            try c.encode("resize", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(cols, forKey: .cols); try c.encode(rows, forKey: .rows)
        case .clipboardSet(let source, let content):
            try c.encode("clipboard_set", forKey: .type)
            try c.encode(source, forKey: .source)
            try c.encode(content, forKey: .content)
        case .watchSession(let sid, let watcherId, let regex, let name):
            try c.encode("watch_session", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(watcherId, forKey: .watcher_id)
            try c.encode(regex, forKey: .regex)
            try c.encode(name, forKey: .name)
        case .unwatchSession(let sid, let watcherId):
            try c.encode("unwatch_session", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(watcherId, forKey: .watcher_id)
        case .watchersList(let sid, let watchers):
            try c.encode("watchers_list", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(watchers, forKey: .watchers)
        case .watcherMatched(let sid, let watcherId, let lineText):
            try c.encode("watcher_matched", forKey: .type)
            try c.encode(sid, forKey: .sid)
            try c.encode(watcherId, forKey: .watcher_id)
            try c.encode(lineText, forKey: .line_text)
        }
    }

    // MARK: - init(from:)

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type_ = try c.decode(String.self, forKey: .type)
        switch type_ {
        case "ping":
            self = .ping(ts: try c.decode(UInt64.self, forKey: .ts),
                         nonce: try c.decode(String.self, forKey: .nonce))
        case "pong":
            self = .pong(ts: try c.decode(UInt64.self, forKey: .ts),
                         nonce: try c.decode(String.self, forKey: .nonce))
        case "heartbeat":
            self = .heartbeat(ts: try c.decode(UInt64.self, forKey: .ts),
                              nonce: try c.decode(String.self, forKey: .nonce))
        case "heartbeat_ack":
            self = .heartbeatAck(ts: try c.decode(UInt64.self, forKey: .ts),
                                 nonce: try c.decode(String.self, forKey: .nonce))
        case "error":
            self = .error(code: try c.decode(String.self, forKey: .code),
                          msg: try c.decode(String.self, forKey: .msg))

        case "launch_session":
            self = .launchSession(
                reqId: try c.decode(String.self, forKey: .req_id),
                launcherId: try c.decode(String.self, forKey: .launcher_id),
                cwdOverride: try c.decodeIfPresent(String.self, forKey: .cwd_override)
            )
        case "launch_ack":
            self = .launchAck(reqId: try c.decode(String.self, forKey: .req_id),
                              sid: try c.decode(String.self, forKey: .sid))
        case "launch_reject":
            self = .launchReject(reqId: try c.decode(String.self, forKey: .req_id),
                                 code: try c.decode(String.self, forKey: .code),
                                 reason: try c.decode(String.self, forKey: .reason))
        case "attach_session":
            self = .attachSession(sid: try c.decode(String.self, forKey: .sid))
        case "detach_session":
            self = .detachSession(sid: try c.decode(String.self, forKey: .sid))
        case "kill_session":
            self = .killSession(sid: try c.decode(String.self, forKey: .sid))
        case "session_list":
            self = .sessionList(sessions: try c.decode([SessionInfo].self, forKey: .sessions))
        case "session_added":
            self = .sessionAdded(session: try c.decode(SessionInfo.self, forKey: .session))
        case "session_removed":
            self = .sessionRemoved(sid: try c.decode(String.self, forKey: .sid),
                                   reason: try c.decode(String.self, forKey: .reason))
        case "session_exited":
            self = .sessionExited(
                sid: try c.decode(String.self, forKey: .sid),
                exitStatus: try c.decodeIfPresent(Int32.self, forKey: .exit_status),
                reason: try c.decode(String.self, forKey: .reason)
            )

        case "term_snapshot":
            self = .termSnapshot(
                sid: try c.decode(String.self, forKey: .sid),
                revision: try c.decode(UInt64.self, forKey: .revision),
                cols: try c.decode(UInt16.self, forKey: .cols),
                rows: try c.decode(UInt16.self, forKey: .rows),
                cursorRow: try c.decode(UInt16.self, forKey: .cursor_row),
                cursorCol: try c.decode(UInt16.self, forKey: .cursor_col),
                cursorVisible: try c.decode(Bool.self, forKey: .cursor_visible),
                title: try c.decodeIfPresent(String.self, forKey: .title),
                lines: try c.decode([TerminalLine].self, forKey: .lines)
            )
        case "term_delta":
            self = .termDelta(
                sid: try c.decode(String.self, forKey: .sid),
                revision: try c.decode(UInt64.self, forKey: .revision),
                cols: try c.decode(UInt16.self, forKey: .cols),
                rows: try c.decode(UInt16.self, forKey: .rows),
                cursorRow: try c.decode(UInt16.self, forKey: .cursor_row),
                cursorCol: try c.decode(UInt16.self, forKey: .cursor_col),
                cursorVisible: try c.decode(Bool.self, forKey: .cursor_visible),
                title: try c.decodeIfPresent(String.self, forKey: .title),
                lines: try c.decode([TerminalLine].self, forKey: .lines)
            )
        case "term_history_snapshot":
            self = .termHistorySnapshot(
                sid: try c.decode(String.self, forKey: .sid),
                revision: try c.decode(UInt64.self, forKey: .revision),
                lines: try c.decode([String].self, forKey: .lines)
            )
        case "term_history_append":
            self = .termHistoryAppend(
                sid: try c.decode(String.self, forKey: .sid),
                revision: try c.decode(UInt64.self, forKey: .revision),
                lines: try c.decode([String].self, forKey: .lines)
            )

        case "input":
            self = .input(sid: try c.decode(String.self, forKey: .sid),
                          payload: try c.decode(TerminalInput.self, forKey: .payload))
        case "resize":
            self = .resize(sid: try c.decode(String.self, forKey: .sid),
                           cols: try c.decode(UInt16.self, forKey: .cols),
                           rows: try c.decode(UInt16.self, forKey: .rows))
        case "clipboard_set":
            self = .clipboardSet(
                source: try c.decode(ClipSource.self, forKey: .source),
                content: try c.decode(ClipContent.self, forKey: .content)
            )

        case "watch_session":
            self = .watchSession(
                sid: try c.decode(String.self, forKey: .sid),
                watcherId: try c.decode(String.self, forKey: .watcher_id),
                regex: try c.decode(String.self, forKey: .regex),
                name: try c.decode(String.self, forKey: .name)
            )
        case "unwatch_session":
            self = .unwatchSession(
                sid: try c.decode(String.self, forKey: .sid),
                watcherId: try c.decode(String.self, forKey: .watcher_id)
            )
        case "watchers_list":
            self = .watchersList(
                sid: try c.decode(String.self, forKey: .sid),
                watchers: try c.decode([WatcherInfo].self, forKey: .watchers)
            )
        case "watcher_matched":
            self = .watcherMatched(
                sid: try c.decode(String.self, forKey: .sid),
                watcherId: try c.decode(String.self, forKey: .watcher_id),
                lineText: try c.decode(String.self, forKey: .line_text)
            )

        default:
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "unknown type \(type_)"
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// SignedCtrl
// ---------------------------------------------------------------------------

struct SignedCtrl: Codable {
    let payload: CtrlPayload
    let sig: String

    enum CodingKeys: String, CodingKey { case sig }

    static func sign(_ p: CtrlPayload, sharedSecret: Data) throws -> SignedCtrl {
        let bytes = try p.canonicalBytes()
        let sig = PairKeys.hmacSign(secret: sharedSecret, message: bytes).base64EncodedString()
        return SignedCtrl(payload: p, sig: sig)
    }

    func verify(sharedSecret: Data) throws {
        guard let sigBytes = Data(base64Encoded: sig) else {
            throw NSError(domain: "Ctrl", code: 1)
        }
        guard PairKeys.hmacVerify(secret: sharedSecret, message: try payload.canonicalBytes(), sig: sigBytes) else {
            throw NSError(domain: "Ctrl", code: 1, userInfo: [NSLocalizedDescriptionKey: "bad sig"])
        }
    }

    func encode(to encoder: Encoder) throws {
        try payload.encode(to: encoder)
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(sig, forKey: .sig)
    }

    init(from decoder: Decoder) throws {
        let payload = try CtrlPayload(from: decoder)
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let sig = try c.decode(String.self, forKey: .sig)
        self.init(payload: payload, sig: sig)
    }

    init(payload: CtrlPayload, sig: String) {
        self.payload = payload
        self.sig = sig
    }
}
