use macagent_core::pair_auth::{derive_shared_secret, PairAuth, PairToken, X25519Pub};

#[test]
fn keypair_round_trip() {
    let pa = PairAuth::generate();
    let pub_b64 = pa.public_key_b64();
    assert_eq!(pub_b64.len() % 4, 0); // base64 4 字符整数倍
    let pub_decoded = X25519Pub::from_b64(&pub_b64).unwrap();
    assert_eq!(pa.public_key_bytes(), *pub_decoded.bytes());
}

#[test]
fn ecdh_derives_same_shared_secret() {
    let mac = PairAuth::generate();
    let ios = PairAuth::generate();
    let s_mac = derive_shared_secret(&mac, &ios.public_key()).unwrap();
    let s_ios = derive_shared_secret(&ios, &mac.public_key()).unwrap();
    assert_eq!(s_mac, s_ios);
    assert_eq!(s_mac.len(), 32);
}

#[test]
fn hmac_sign_verify_round_trip() {
    let mac = PairAuth::generate();
    let ios = PairAuth::generate();
    let s = derive_shared_secret(&mac, &ios.public_key()).unwrap();
    let sig = macagent_core::pair_auth::hmac_sign(&s, b"hello world");
    assert!(macagent_core::pair_auth::hmac_verify(
        &s,
        b"hello world",
        &sig
    ));
    assert!(!macagent_core::pair_auth::hmac_verify(
        &s,
        b"hello world!",
        &sig
    ));
}

#[test]
fn pair_token_struct_serializes() {
    let tok = PairToken {
        pair_token: "ABC234".into(),
        room_id: "11111111-1111-1111-1111-111111111111".into(),
        worker_url: "https://macagent.workers.dev".into(),
        mac_device_secret: "ZmFrZS0zMmJ5dGUtbWFjLWRldmljZS1zZWNyZXQ=".into(),
    };
    let json = serde_json::to_string(&tok).unwrap();
    let back: PairToken = serde_json::from_str(&json).unwrap();
    assert_eq!(tok.pair_token, back.pair_token);
}
