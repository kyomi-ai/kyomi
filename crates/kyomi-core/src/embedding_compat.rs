// SPDX-License-Identifier: AGPL-3.0-or-later

//! Embedding storage compatibility — conversion between f32 vectors and byte storage.
//!
//! Postgres stores embeddings as `vector(384)` via pgvector.
//! SQLite stores embeddings as `BLOB` (raw f32 little-endian bytes, 384 × 4 = 1536 bytes).
//! Model structs use `Vec<u8>` as the universal representation.

/// Convert f32 slice to bytes for storage.
///
/// Works as BLOB in SQLite, and can be cast to pgvector::Vector for Postgres vector queries.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert stored bytes back to f32 vector.
///
/// Used when loading embeddings for in-memory vector search (SQLite)
/// or converting to pgvector::Vector for Postgres vector queries.
pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    debug_assert!(
        bytes.len().is_multiple_of(4),
        "embedding blob length {} is not a multiple of 4",
        bytes.len()
    );
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_conversion() {
        let original = vec![1.0f32, -2.5, 0.0, 3.14159, f32::MIN, f32::MAX];
        let bytes = embedding_to_bytes(&original);
        assert_eq!(bytes.len(), original.len() * 4);
        let restored = bytes_to_embedding(&bytes);
        assert_eq!(original, restored);
    }

    #[test]
    fn empty_embedding() {
        let bytes = embedding_to_bytes(&[]);
        assert!(bytes.is_empty());
        let restored = bytes_to_embedding(&bytes);
        assert!(restored.is_empty());
    }

    #[test]
    fn correct_byte_count_384dim() {
        let embedding = vec![0.0f32; 384];
        let bytes = embedding_to_bytes(&embedding);
        assert_eq!(bytes.len(), 384 * 4);
        assert_eq!(bytes.len(), 1536);
    }
}
