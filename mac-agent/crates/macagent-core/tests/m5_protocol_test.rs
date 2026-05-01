use macagent_core::ctrl_msg::{
    canonical_bytes, sign, verify, CtrlPayload, SupervisionEntry, SupervisionSource,
    SupervisionStatus, Viewport, WindowInfo,
};

#[test]
fn windows_list_round_trip() {
    let p = CtrlPayload::WindowsList {
        windows: vec![WindowInfo {
            window_id: 42,
            app_name: "Chrome".into(),
            bundle_id: Some("com.google.Chrome".into()),
            title: "GitHub".into(),
            width: 1440,
            height: 900,
            on_screen: true,
            is_minimized: false,
        }],
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn supervise_existing_with_viewport_round_trip() {
    let p = CtrlPayload::SuperviseExisting {
        window_id: 42,
        viewport: Viewport {
            width: 393,
            height: 852,
        },
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn supervised_ack_canonical_bytes_sorted_recursively() {
    let p = CtrlPayload::SupervisedAck {
        sup_id: "abc".into(),
        entry: SupervisionEntry {
            sup_id: "abc".into(),
            window_id: 42,
            app_name: "Chrome".into(),
            title: "GH".into(),
            status: SupervisionStatus::Active,
            source: SupervisionSource::Existing,
            started_ts: 1735200000000,
        },
    };
    let bytes = canonical_bytes(&p);
    let s = std::str::from_utf8(&bytes).unwrap();
    // nested entry keys must also be sorted: app_name < sup_id < window_id
    let app_pos = s.find("app_name").unwrap();
    let entry_sup_pos = s.rfind("\"sup_id\"").unwrap(); // entry.sup_id (not outer sup_id)
    assert!(app_pos < entry_sup_pos);
}

#[test]
fn sign_and_verify_supervised_ack_with_nested() {
    let p = CtrlPayload::SupervisedAck {
        sup_id: "abc".into(),
        entry: SupervisionEntry {
            sup_id: "abc".into(),
            window_id: 1,
            app_name: "X".into(),
            title: "Y".into(),
            status: SupervisionStatus::Active,
            source: SupervisionSource::Existing,
            started_ts: 0,
        },
    };
    let secret = [0xAB; 32];
    let signed = sign(p.clone(), &secret);
    verify(&signed, &secret).expect("verify should pass");
}

#[test]
fn stream_ended_round_trip() {
    let p = CtrlPayload::StreamEnded {
        sup_id: "abc".into(),
        reason: "window_closed".into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}
