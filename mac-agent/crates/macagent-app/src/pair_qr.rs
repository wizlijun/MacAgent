//! QR code PNG generation for the pair payload.

use anyhow::Result;
use qrcode::QrCode;

pub fn encode_pair_qr_png(payload_json: &str) -> Result<Vec<u8>> {
    let code = QrCode::new(payload_json.as_bytes())?;
    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(256, 256)
        .build();
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_round_trip() {
        let png = super::encode_pair_qr_png(r#"{"pair_token":"ABC234"}"#).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
