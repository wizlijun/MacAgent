use macagent_core::ctrl_msg::{canonical_bytes, CtrlPayload, GuiInput, KeyMod};

#[test]
fn gui_input_cmd_round_trip() {
    let payload = CtrlPayload::GuiInputCmd {
        sup_id: "abc".into(),
        payload: GuiInput::Tap { x: 0.5, y: 0.25 },
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload, back);
}

#[test]
fn gui_input_keycombo_signature_canonical() {
    let payload = CtrlPayload::GuiInputCmd {
        sup_id: "abc".into(),
        payload: GuiInput::KeyCombo {
            modifiers: vec![KeyMod::Cmd, KeyMod::Shift],
            key: "p".into(),
        },
    };
    let bytes = canonical_bytes(&payload);
    // Stable shape: nested Vec/Map sorted recursively
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("\"modifiers\""));
    assert!(s.contains("\"cmd\""));
    assert!(s.contains("\"shift\""));
}

#[test]
fn gui_input_ack_optional_message() {
    let payload = CtrlPayload::GuiInputAck {
        sup_id: "abc".into(),
        code: "ok".into(),
        message: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload, back);
}
