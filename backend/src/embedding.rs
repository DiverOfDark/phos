/// Encode an f32 embedding as raw little-endian bytes (no length prefix).
pub fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &v in embedding {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

/// Decode an embedding blob, supporting both raw LE f32 and legacy bincode format.
///
/// Legacy bincode format: 8-byte u64 LE length prefix + N×4 bytes of LE f32.
/// New raw format: N×4 bytes of LE f32 (no prefix).
pub fn decode_embedding(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() {
        return None;
    }

    // Try legacy bincode format first: 8-byte u64 LE prefix where prefix * 4 + 8 == blob.len()
    if blob.len() >= 12 {
        let prefix = u64::from_le_bytes(blob[..8].try_into().ok()?) as usize;
        if prefix > 0 && prefix.checked_mul(4).and_then(|v| v.checked_add(8)) == Some(blob.len()) {
            let data = &blob[8..];
            let floats: Vec<f32> = data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            return Some(floats);
        }
    }

    // Raw format: blob length must be a multiple of 4
    if blob.len() % 4 != 0 {
        return None;
    }

    let floats: Vec<f32> = blob
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Some(floats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let embedding = vec![1.0f32, -2.5, 0.0, 3.14, f32::MAX, f32::MIN];
        let blob = encode_embedding(&embedding);
        assert_eq!(blob.len(), embedding.len() * 4);
        let decoded = decode_embedding(&blob).unwrap();
        assert_eq!(embedding, decoded);
    }

    #[test]
    fn decode_legacy_bincode_format() {
        let embedding = vec![1.0f32, 2.0, 3.0];
        // Legacy format: 8-byte LE u64 length prefix (element count) + raw f32 data
        let mut blob = (embedding.len() as u64).to_le_bytes().to_vec();
        for &v in &embedding {
            blob.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(blob.len(), 8 + 3 * 4); // 20 bytes
        let decoded = decode_embedding(&blob).unwrap();
        assert_eq!(embedding, decoded);
    }

    #[test]
    fn decode_legacy_512_dim() {
        let embedding: Vec<f32> = (0..512).map(|i| i as f32 * 0.001).collect();
        let mut blob = (512u64).to_le_bytes().to_vec();
        for &v in &embedding {
            blob.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(blob.len(), 2056);
        let decoded = decode_embedding(&blob).unwrap();
        assert_eq!(embedding, decoded);
    }

    #[test]
    fn decode_empty_returns_none() {
        assert!(decode_embedding(&[]).is_none());
    }

    #[test]
    fn decode_invalid_length_returns_none() {
        assert!(decode_embedding(&[1, 2, 3]).is_none());
        assert!(decode_embedding(&[1, 2, 3, 4, 5]).is_none());
    }
}
