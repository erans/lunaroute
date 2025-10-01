//! Encryption utilities for session storage

use crate::traits::{StorageError, StorageResult};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

/// Encrypt data using AES-256-GCM
pub fn encrypt(data: &[u8], key: &[u8; 32]) -> StorageResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());

    // Generate random nonce
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| StorageError::Serialization(format!("Encryption error: {}", e)))?;

    // Prepend nonce to ciphertext
    let mut result = nonce_bytes.to_vec();
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt data using AES-256-GCM
pub fn decrypt(encrypted_data: &[u8], key: &[u8; 32]) -> StorageResult<Vec<u8>> {
    if encrypted_data.len() < 12 {
        return Err(StorageError::InvalidData(
            "Encrypted data too short".to_string(),
        ));
    }

    let cipher = Aes256Gcm::new(key.into());

    // Extract nonce from the beginning
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| StorageError::Serialization(format!("Decryption error: {}", e)))?;

    Ok(plaintext)
}

/// Generate a random encryption key
pub fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_roundtrip() {
        let key = generate_key();
        let data = b"Secret message that needs encryption!";

        let encrypted = encrypt(data, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(data, decrypted.as_slice());
        assert_ne!(data.as_slice(), &encrypted[12..]); // Ciphertext should be different
    }

    #[test]
    fn test_encryption_different_nonces() {
        let key = generate_key();
        let data = b"Test data";

        let encrypted1 = encrypt(data, &key).unwrap();
        let encrypted2 = encrypt(data, &key).unwrap();

        // Same plaintext encrypted twice should produce different ciphertext (different nonces)
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same plaintext
        let decrypted1 = decrypt(&encrypted1, &key).unwrap();
        let decrypted2 = decrypt(&encrypted2, &key).unwrap();
        assert_eq!(decrypted1, decrypted2);
        assert_eq!(data, decrypted1.as_slice());
    }

    #[test]
    fn test_encryption_wrong_key() {
        let key1 = generate_key();
        let key2 = generate_key();
        let data = b"Secret data";

        let encrypted = encrypt(data, &key1).unwrap();
        let result = decrypt(&encrypted, &key2);

        assert!(result.is_err());
    }

    #[test]
    fn test_encryption_corrupted_data() {
        let key = generate_key();
        let data = b"Test data";

        let mut encrypted = encrypt(data, &key).unwrap();

        // Corrupt some bytes in the ciphertext
        if encrypted.len() > 20 {
            encrypted[20] ^= 0xFF;
        }

        let result = decrypt(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encryption_empty_data() {
        let key = generate_key();
        let data = b"";

        let encrypted = encrypt(data, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(data, decrypted.as_slice());
    }

    #[test]
    fn test_encryption_large_data() {
        let key = generate_key();
        let data = vec![0x42u8; 1024 * 1024]; // 1MB

        let encrypted = encrypt(&data, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_generate_key_uniqueness() {
        let key1 = generate_key();
        let key2 = generate_key();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_decrypt_too_short() {
        let key = generate_key();
        let short_data = vec![0u8; 10]; // Less than 12 bytes

        let result = decrypt(&short_data, &key);
        assert!(result.is_err());
    }
}
