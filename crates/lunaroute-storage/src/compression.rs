//! Compression utilities for session storage

use crate::traits::{StorageError, StorageResult};
use std::io::{Read, Write};

/// Compression algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    /// Zstandard compression (best compression ratio)
    Zstd,
    /// LZ4 compression (fastest)
    Lz4,
    /// No compression
    None,
}

/// Compress data using the specified algorithm
pub fn compress(data: &[u8], algorithm: CompressionAlgorithm) -> StorageResult<Vec<u8>> {
    match algorithm {
        CompressionAlgorithm::Zstd => compress_zstd(data),
        CompressionAlgorithm::Lz4 => compress_lz4(data),
        CompressionAlgorithm::None => Ok(data.to_vec()),
    }
}

/// Decompress data using the specified algorithm
pub fn decompress(data: &[u8], algorithm: CompressionAlgorithm) -> StorageResult<Vec<u8>> {
    match algorithm {
        CompressionAlgorithm::Zstd => decompress_zstd(data),
        CompressionAlgorithm::Lz4 => decompress_lz4(data),
        CompressionAlgorithm::None => Ok(data.to_vec()),
    }
}

/// Compress data using Zstandard
fn compress_zstd(data: &[u8]) -> StorageResult<Vec<u8>> {
    let mut encoder = zstd::Encoder::new(Vec::new(), 3)
        .map_err(|e| StorageError::Serialization(format!("Zstd encoder error: {}", e)))?;

    encoder
        .write_all(data)
        .map_err(|e| StorageError::Serialization(format!("Zstd write error: {}", e)))?;

    encoder
        .finish()
        .map_err(|e| StorageError::Serialization(format!("Zstd finish error: {}", e)))
}

/// Decompress data using Zstandard
fn decompress_zstd(data: &[u8]) -> StorageResult<Vec<u8>> {
    let mut decoder = zstd::Decoder::new(data)
        .map_err(|e| StorageError::Serialization(format!("Zstd decoder error: {}", e)))?;

    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| StorageError::Serialization(format!("Zstd read error: {}", e)))?;

    Ok(decompressed)
}

/// Compress data using LZ4
fn compress_lz4(data: &[u8]) -> StorageResult<Vec<u8>> {
    let mut encoder = lz4::EncoderBuilder::new()
        .build(Vec::new())
        .map_err(|e| StorageError::Serialization(format!("LZ4 encoder error: {}", e)))?;

    encoder
        .write_all(data)
        .map_err(|e| StorageError::Serialization(format!("LZ4 write error: {}", e)))?;

    let (compressed, result) = encoder.finish();
    result.map_err(|e| StorageError::Serialization(format!("LZ4 finish error: {}", e)))?;

    Ok(compressed)
}

/// Decompress data using LZ4
fn decompress_lz4(data: &[u8]) -> StorageResult<Vec<u8>> {
    let mut decoder = lz4::Decoder::new(data)
        .map_err(|e| StorageError::Serialization(format!("LZ4 decoder error: {}", e)))?;

    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| StorageError::Serialization(format!("LZ4 read error: {}", e)))?;

    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zstd_compression_roundtrip() {
        let data = b"Hello, world! This is a test of Zstandard compression.";
        let compressed = compress_zstd(data).unwrap();
        let decompressed = decompress_zstd(&compressed).unwrap();

        assert_eq!(data, decompressed.as_slice());
        // Note: Small data may not compress well due to headers
    }

    #[test]
    fn test_lz4_compression_roundtrip() {
        let data = b"Hello, world! This is a test of LZ4 compression.";
        let compressed = compress_lz4(data).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();

        assert_eq!(data, decompressed.as_slice());
    }

    #[test]
    fn test_compression_with_algorithm() {
        let data = b"Test data for compression algorithms";

        // Zstd
        let zstd_compressed = compress(data, CompressionAlgorithm::Zstd).unwrap();
        let zstd_decompressed = decompress(&zstd_compressed, CompressionAlgorithm::Zstd).unwrap();
        assert_eq!(data, zstd_decompressed.as_slice());

        // LZ4
        let lz4_compressed = compress(data, CompressionAlgorithm::Lz4).unwrap();
        let lz4_decompressed = decompress(&lz4_compressed, CompressionAlgorithm::Lz4).unwrap();
        assert_eq!(data, lz4_decompressed.as_slice());

        // None
        let none_compressed = compress(data, CompressionAlgorithm::None).unwrap();
        assert_eq!(data, none_compressed.as_slice());
    }

    #[test]
    fn test_large_data_compression() {
        let data = vec![0u8; 1024 * 1024]; // 1MB of zeros

        let compressed = compress(&data, CompressionAlgorithm::Zstd).unwrap();
        let decompressed = decompress(&compressed, CompressionAlgorithm::Zstd).unwrap();

        assert_eq!(data, decompressed);
        assert!(compressed.len() < data.len() / 100); // Should compress to < 1%
    }

    #[test]
    fn test_compression_algorithm_equality() {
        assert_eq!(CompressionAlgorithm::Zstd, CompressionAlgorithm::Zstd);
        assert_ne!(CompressionAlgorithm::Zstd, CompressionAlgorithm::Lz4);
    }
}
