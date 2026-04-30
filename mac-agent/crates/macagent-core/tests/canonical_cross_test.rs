use macagent_core::ctrl_msg::{
    canonical_bytes, sign, verify, CtrlPayload, InputKey, TerminalInput,
};

#[test]
fn nested_payload_canonical_order_independent() {
    let p = CtrlPayload::Input {
        sid: "s1".into(),
        payload: TerminalInput::Key {
            key: InputKey::CtrlC,
        },
    };
    let b = canonical_bytes(&p);
    let s = std::str::from_utf8(&b).unwrap();
    // Nested keys must be sorted: "key" before "kind"
    assert!(
        s.contains(r#""payload":{"key":"ctrl_c","kind":"key"}"#),
        "expected nested keys sorted, got: {}",
        s
    );
    // Top-level key order (BTreeMap alphabetical): payload < sid < type
    let payload_pos = s.find("\"payload\"").unwrap();
    let sid_pos = s.find("\"sid\"").unwrap();
    let type_pos = s.find("\"type\"").unwrap();
    assert!(payload_pos < sid_pos, "payload should come before sid");
    assert!(sid_pos < type_pos, "sid should come before type");
}

#[test]
fn sign_and_verify_with_nested_payload_round_trip() {
    let p = CtrlPayload::Input {
        sid: "abc".into(),
        payload: TerminalInput::Text {
            data: "hello".into(),
        },
    };
    let secret = [0xABu8; 32];
    let signed = sign(p, &secret);
    verify(&signed, &secret).expect("verify should pass");
}
