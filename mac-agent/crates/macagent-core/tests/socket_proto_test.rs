use bytes::BytesMut;
use macagent_core::ctrl_msg::{InputKey, SessionSource, TerminalInput};
use macagent_core::socket_proto::{codec, A2P, P2A};

#[test]
fn round_trip_p2a_producer_hello() {
    let msg = P2A::ProducerHello {
        argv: vec!["zsh".to_string(), "-l".to_string()],
        pid: 4242,
        cwd: Some("/Users/test".to_string()),
        cols: 80,
        rows: 24,
        source: SessionSource::IosLaunched {
            launcher_id: "zsh".to_string(),
        },
    };
    let mut buf = codec::encode(&msg).unwrap();
    let decoded: P2A = codec::try_decode(&mut buf).unwrap().unwrap();
    assert!(buf.is_empty());
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        serde_json::to_string(&decoded).unwrap()
    );
}

#[test]
fn round_trip_a2p_input() {
    let msg = A2P::Input {
        payload: TerminalInput::Key {
            key: InputKey::Enter,
        },
    };
    let mut buf = codec::encode(&msg).unwrap();
    let decoded: A2P = codec::try_decode(&mut buf).unwrap().unwrap();
    assert!(buf.is_empty());
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        serde_json::to_string(&decoded).unwrap()
    );
}

#[test]
fn round_trip_a2p_resize() {
    let msg = A2P::Resize {
        cols: 120,
        rows: 40,
    };
    let mut buf = codec::encode(&msg).unwrap();
    let decoded: A2P = codec::try_decode(&mut buf).unwrap().unwrap();
    assert!(buf.is_empty());
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        serde_json::to_string(&decoded).unwrap()
    );
}

#[test]
fn partial_frame_no_header_returns_none() {
    let msg = A2P::ProducerWelcome {
        sid: "s1".to_string(),
    };
    let full = codec::encode(&msg).unwrap();
    // Only 3 bytes: not enough for 4-byte header
    let mut partial = BytesMut::from(&full[..3]);
    let result: Option<A2P> = codec::try_decode(&mut partial).unwrap();
    assert!(result.is_none());
    // Buffer must be unchanged
    assert_eq!(partial.len(), 3);
}

#[test]
fn partial_frame_header_only_returns_none() {
    let msg = A2P::ProducerWelcome {
        sid: "s1".to_string(),
    };
    let full = codec::encode(&msg).unwrap();
    // 4 bytes header + 1 body byte: incomplete
    let mut partial = BytesMut::from(&full[..5]);
    let result: Option<A2P> = codec::try_decode(&mut partial).unwrap();
    assert!(result.is_none());
    assert_eq!(partial.len(), 5);
}

#[test]
fn round_trip_p2a_producer_exit() {
    let msg = P2A::ProducerExit {
        exit_status: Some(0),
        reason: "done".to_string(),
    };
    let mut buf = codec::encode(&msg).unwrap();
    let decoded: P2A = codec::try_decode(&mut buf).unwrap().unwrap();
    assert!(buf.is_empty());
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        serde_json::to_string(&decoded).unwrap()
    );
}

#[test]
fn round_trip_a2p_attach_start() {
    let msg = A2P::AttachStart;
    let mut buf = codec::encode(&msg).unwrap();
    let decoded: A2P = codec::try_decode(&mut buf).unwrap().unwrap();
    assert!(buf.is_empty());
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        serde_json::to_string(&decoded).unwrap()
    );
}
