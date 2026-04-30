use macagent_core::pair_auth::PairAuth;
use macagent_core::signaling::{SignalingClient, WsAuthQuery};
use tokio::net::TcpListener;

#[tokio::test(flavor = "current_thread")]
async fn ws_auth_query_signs_correctly() {
    let _pa = PairAuth::generate();
    let secret = [1u8; 32];
    let q = WsAuthQuery::build("mac", "pair-id-1", 1234567890, "noncebytes", &secret);
    assert!(q.contains("device=mac"));
    assert!(q.contains("pair_id=pair-id-1"));
    assert!(q.contains("ts=1234567890"));
    assert!(q.contains("nonce=noncebytes"));
    assert!(q.contains("sig="));
}

#[tokio::test(flavor = "current_thread")]
async fn dial_and_echo() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            use futures_util::{SinkExt, StreamExt};
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_text() {
                    ws.send(msg).await.ok();
                }
            }
        }
    });

    let url = format!("ws://127.0.0.1:{port}/signal/test?device=mac&ts=0&nonce=x&sig=x");
    let mut client = SignalingClient::connect(&url).await.unwrap();
    client.send_text("hello").await.unwrap();
    let echoed = client.recv_text().await.unwrap();
    assert_eq!(echoed, "hello");
}
