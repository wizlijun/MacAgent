use macagent_core::ctrl_msg::{
    canonical_bytes, CtrlPayload, InputKey, SessionInfo, SessionSource, TerminalColor,
    TerminalInput, TerminalLine, TerminalRun,
};

fn make_session_info() -> SessionInfo {
    SessionInfo {
        sid: "sid-001".to_string(),
        label: "Zsh".to_string(),
        argv: vec!["zsh".to_string(), "-l".to_string()],
        pid: 9999,
        cols: 80,
        rows: 24,
        started_ts: 1700000000,
        streaming: true,
        source: SessionSource::IosLaunched {
            launcher_id: "zsh".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// JSON round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn round_trip_launch_session() {
    let p = CtrlPayload::LaunchSession {
        req_id: "req-1".to_string(),
        launcher_id: "zsh".to_string(),
        cwd_override: Some("/tmp".to_string()),
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

#[test]
fn round_trip_session_list() {
    let p = CtrlPayload::SessionList {
        sessions: vec![make_session_info()],
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

#[test]
fn round_trip_session_exited() {
    let p = CtrlPayload::SessionExited {
        sid: "sid-001".to_string(),
        exit_status: Some(1),
        reason: "process exited".to_string(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

#[test]
fn round_trip_input_key() {
    let p = CtrlPayload::Input {
        sid: "sid-001".to_string(),
        payload: TerminalInput::Key {
            key: InputKey::CtrlC,
        },
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

#[test]
fn round_trip_input_text() {
    let p = CtrlPayload::Input {
        sid: "sid-001".to_string(),
        payload: TerminalInput::Text {
            data: "ls -la\n".to_string(),
        },
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

#[test]
fn round_trip_resize() {
    let p = CtrlPayload::Resize {
        sid: "sid-001".to_string(),
        cols: 120,
        rows: 40,
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}

// ---------------------------------------------------------------------------
// canonical_bytes test
// ---------------------------------------------------------------------------

#[test]
fn canonical_bytes_ping_matches_expected() {
    let p = CtrlPayload::Ping {
        ts: 1000,
        nonce: "abc".to_string(),
    };
    let bytes = canonical_bytes(&p);
    // Keys must be sorted alphabetically: nonce, ts, type
    let expected = br#"{"nonce":"abc","ts":1000,"type":"ping"}"#;
    assert_eq!(bytes, expected);
}

#[test]
fn canonical_bytes_launch_session_sorted() {
    let p = CtrlPayload::LaunchSession {
        req_id: "r1".to_string(),
        launcher_id: "zsh".to_string(),
        cwd_override: None,
    };
    let bytes = canonical_bytes(&p);
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let obj = parsed.as_object().unwrap();
    let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
    // Verify keys are sorted
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

// ---------------------------------------------------------------------------
// TermSnapshot large object round-trip
// ---------------------------------------------------------------------------

#[test]
fn round_trip_term_snapshot_large() {
    let lines: Vec<TerminalLine> = (0u16..24)
        .map(|i| TerminalLine {
            index: i,
            runs: vec![
                TerminalRun {
                    text: format!("line {} content", i),
                    fg: Some(TerminalColor::Indexed { value: 2 }),
                    bg: None,
                    bold: false,
                    dim: false,
                    italic: false,
                    underline: false,
                    inverse: false,
                },
                TerminalRun {
                    text: " extra".to_string(),
                    fg: Some(TerminalColor::Rgb {
                        r: 255,
                        g: 0,
                        b: 128,
                    }),
                    bg: Some(TerminalColor::Indexed { value: 0 }),
                    bold: true,
                    dim: false,
                    italic: true,
                    underline: false,
                    inverse: false,
                },
            ],
            wrapped: i % 3 == 0,
        })
        .collect();

    let p = CtrlPayload::TermSnapshot {
        sid: "sid-999".to_string(),
        revision: 42,
        cols: 80,
        rows: 24,
        cursor_row: 10,
        cursor_col: 5,
        cursor_visible: true,
        title: Some("bash".to_string()),
        lines,
    };
    let json = serde_json::to_string(&p).unwrap();
    let p2: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, p2);
}
