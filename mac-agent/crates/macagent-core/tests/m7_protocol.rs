use macagent_core::ctrl_msg::{
    canonical_bytes, CtrlPayload, SupStatus, SupervisionEntry, Viewport, WindowRect,
};

#[test]
fn supervise_launch_round_trip() {
    let p = CtrlPayload::SuperviseLaunch {
        bundle_id: "com.anthropic.claude".into(),
        viewport: Viewport { w: 393, h: 760 },
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn switch_active_canonical_sorted() {
    let p = CtrlPayload::SwitchActive {
        sup_id: "abc".into(),
        viewport: Viewport { w: 768, h: 1024 },
    };
    let bytes = canonical_bytes(&p);
    let s = std::str::from_utf8(&bytes).unwrap();
    // Top-level: sup_id < type < viewport (lexicographic)
    assert!(s.find("\"sup_id\"").unwrap() < s.find("\"type\"").unwrap());
    assert!(s.find("\"type\"").unwrap() < s.find("\"viewport\"").unwrap());
    // Nested viewport: h < w
    assert!(s.find("\"h\"").unwrap() < s.find("\"w\"").unwrap());
}

#[test]
fn viewport_changed_round_trip() {
    let p = CtrlPayload::ViewportChanged {
        sup_id: "abc".into(),
        viewport: Viewport { w: 100, h: 200 },
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn fit_failed_round_trip() {
    let p = CtrlPayload::FitFailed {
        sup_id: "abc".into(),
        reason: "ax_denied".into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn supervision_entry_extended_fields() {
    let entry = SupervisionEntry {
        sup_id: "abc".into(),
        window_id: 123,
        app_name: "Claude".into(),
        title: "Chat".into(),
        width: 1440,
        height: 900,
        status: SupStatus::Armed,
        original_frame: Some(WindowRect {
            x: 100,
            y: 100,
            w: 1440,
            h: 900,
        }),
        thumb_jpeg_b64: Some("AAAA".into()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: SupervisionEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(entry, back);
}

#[test]
fn sup_status_lowercase() {
    assert_eq!(serde_json::to_string(&SupStatus::Active).unwrap(), "\"active\"");
    assert_eq!(serde_json::to_string(&SupStatus::Armed).unwrap(), "\"armed\"");
    assert_eq!(serde_json::to_string(&SupStatus::Dead).unwrap(), "\"dead\"");
}
