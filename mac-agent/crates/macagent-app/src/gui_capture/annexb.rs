//! AVCC (length-prefix) ↔ Annex-B (start-code prefix) for H.264 NAL streams.

use anyhow::{anyhow, Result};
use bytes::{BufMut, Bytes, BytesMut};

const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

/// Rewrite `[len_be: u32][nal: len bytes]` records into `[0x00,0x00,0x00,0x01][nal]`.
pub fn avcc_to_annexb(avcc: &[u8]) -> Result<Bytes> {
    let mut out = BytesMut::with_capacity(avcc.len() + 16);
    let mut i = 0;
    while i < avcc.len() {
        if i + 4 > avcc.len() {
            return Err(anyhow!(
                "avcc: truncated length prefix at offset {} (len {})",
                i,
                avcc.len()
            ));
        }
        let len = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        let end = i.checked_add(len).ok_or_else(|| anyhow!("avcc: length overflow"))?;
        if end > avcc.len() {
            return Err(anyhow!(
                "avcc: NAL of length {} runs past end (offset {}, buf {})",
                len,
                i,
                avcc.len()
            ));
        }
        out.put_slice(START_CODE);
        out.put_slice(&avcc[i..end]);
        i = end;
    }
    Ok(out.freeze())
}

/// Build a keyframe sample: SPS + PPS + VCL NALs, all in Annex-B.
pub fn build_keyframe(sps: &[u8], pps: &[u8], vcl_avcc: &[u8]) -> Result<Bytes> {
    let vcl = avcc_to_annexb(vcl_avcc)?;
    let mut out = BytesMut::with_capacity(8 + sps.len() + pps.len() + vcl.len());
    out.put_slice(START_CODE);
    out.put_slice(sps);
    out.put_slice(START_CODE);
    out.put_slice(pps);
    out.put_slice(&vcl);
    Ok(out.freeze())
}

/// Build a non-keyframe sample: just VCL NALs (no SPS/PPS).
pub fn build_inter(vcl_avcc: &[u8]) -> Result<Bytes> {
    avcc_to_annexb(vcl_avcc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_to_annexb_two_nals() {
        let input: &[u8] = &[
            0, 0, 0, 2, 0x67, 0x42, 0, 0, 0, 3, 0x68, 0xCE, 0x06,
        ];
        let expected: &[u8] = &[
            0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0x06,
        ];
        let out = avcc_to_annexb(input).expect("should convert");
        assert_eq!(out.as_ref(), expected);
    }

    #[test]
    fn build_keyframe_layout() {
        let sps: &[u8] = &[0x67, 0x01];
        let pps: &[u8] = &[0x68, 0x02];
        let vcl: &[u8] = &[0, 0, 0, 2, 0x65, 0x88];
        let expected: &[u8] = &[
            0, 0, 0, 1, 0x67, 0x01, 0, 0, 0, 1, 0x68, 0x02, 0, 0, 0, 1, 0x65, 0x88,
        ];
        let out = build_keyframe(sps, pps, vcl).expect("should build");
        assert_eq!(out.as_ref(), expected);
    }

    #[test]
    fn avcc_malformed_overrun() {
        let input: &[u8] = &[0, 0, 0, 5, 0x67, 0x42];
        assert!(avcc_to_annexb(input).is_err());
    }

    #[test]
    fn avcc_empty_input() {
        let out = avcc_to_annexb(&[]).expect("empty should be ok");
        assert!(out.is_empty());
    }
}
