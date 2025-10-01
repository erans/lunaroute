//! Encryption utilities for session storage

use crate::traits::{StorageError, StorageResult};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2, ParamsBuilder, Version,
};
use rand::RngCore;

/// Key derivation parameters for Argon2id
#[derive(Debug, Clone)]
pub struct KeyDerivationParams {
    /// Memory size in KiB (default: 64MB)
    pub memory_size: u32,
    /// Number of iterations (default: 3)
    pub iterations: u32,
    /// Degree of parallelism (default: 4)
    pub parallelism: u32,
}

impl Default for KeyDerivationParams {
    fn default() -> Self {
        Self {
            memory_size: 65536, // 64 MB
            iterations: 3,      // OWASP recommended minimum
            parallelism: 4,     // Use 4 threads
        }
    }
}

/// Derive an encryption key from a password using Argon2id
pub fn derive_key_from_password(
    password: &str,
    salt: &[u8; 16],
    params: &KeyDerivationParams,
) -> StorageResult<[u8; 32]> {
    // Create Argon2 params
    let argon2_params = ParamsBuilder::new()
        .m_cost(params.memory_size)
        .t_cost(params.iterations)
        .p_cost(params.parallelism)
        .build()
        .map_err(|e| StorageError::InvalidData(format!("Invalid Argon2 params: {}", e)))?;

    // Use Argon2id (most secure variant)
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        Version::V0x13,
        argon2_params,
    );

    let mut key = [0u8; 32];

    // Derive key
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| StorageError::Serialization(format!("Key derivation failed: {}", e)))?;

    Ok(key)
}

/// Generate a random salt for key derivation
pub fn generate_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    salt
}

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

    #[test]
    fn test_key_derivation_deterministic() {
        let password = "my-secure-password";
        let salt = [0u8; 16]; // Fixed salt for deterministic test
        let params = KeyDerivationParams::default();

        let key1 = derive_key_from_password(password, &salt, &params).unwrap();
        let key2 = derive_key_from_password(password, &salt, &params).unwrap();

        // Same password + salt should produce same key
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_key_derivation_different_passwords() {
        let salt = [0u8; 16];
        let params = KeyDerivationParams::default();

        let key1 = derive_key_from_password("password1", &salt, &params).unwrap();
        let key2 = derive_key_from_password("password2", &salt, &params).unwrap();

        // Different passwords should produce different keys
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_key_derivation_different_salts() {
        let password = "my-password";
        let salt1 = [0u8; 16];
        let salt2 = [1u8; 16];
        let params = KeyDerivationParams::default();

        let key1 = derive_key_from_password(password, &salt1, &params).unwrap();
        let key2 = derive_key_from_password(password, &salt2, &params).unwrap();

        // Different salts should produce different keys
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_generate_salt_uniqueness() {
        let salt1 = generate_salt();
        let salt2 = generate_salt();

        assert_ne!(salt1, salt2);
    }

    #[test]
    fn test_encrypt_decrypt_with_derived_key() {
        let password = "test-password-123";
        let salt = generate_salt();
        let params = KeyDerivationParams::default();

        let key = derive_key_from_password(password, &salt, &params).unwrap();

        let data = b"Test data for derived key encryption";
        let encrypted = encrypt(data, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(data, decrypted.as_slice());
    }

    #[test]
    fn test_key_derivation_custom_params() {
        let password = "test";
        let salt = [0u8; 16];

        // Lighter params for faster testing
        let params = KeyDerivationParams {
            memory_size: 4096,   // 4 MB
            iterations: 2,
            parallelism: 1,
        };

        let key = derive_key_from_password(password, &salt, &params);
        assert!(key.is_ok());
    }
}
