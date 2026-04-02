/// Encode an f32 embedding as raw little-endian bytes (no length prefix).
pub fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &v in embedding {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

/// Decode an embedding blob from raw little-endian f32 bytes.
pub fn decode_embedding(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || blob.len() % 4 != 0 {
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
    fn decode_empty_returns_none() {
        assert!(decode_embedding(&[]).is_none());
    }

    #[test]
    fn decode_invalid_length_returns_none() {
        assert!(decode_embedding(&[1, 2, 3]).is_none());
        assert!(decode_embedding(&[1, 2, 3, 4, 5]).is_none());
    }
}
