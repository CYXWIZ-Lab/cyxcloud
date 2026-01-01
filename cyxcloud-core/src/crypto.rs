//! Cryptographic primitives for CyxCloud
//!
//! Provides:
//! - Blake3 content hashing (fast, parallelizable)
//! - AES-256-GCM encryption (authenticated encryption)
//! - Key derivation using Argon2

use crate::error::{CyxCloudError, Result};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// AES-256-GCM key size (32 bytes)
pub const KEY_SIZE: usize = 32;

/// AES-GCM nonce size (12 bytes / 96 bits)
pub const NONCE_SIZE: usize = 12;

/// AES-GCM authentication tag size (16 bytes)
pub const TAG_SIZE: usize = 16;

/// Blake3 hash wrapper for content addressing
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(blake3::Hash);

impl ContentHash {
    /// Compute Blake3 hash of data
    pub fn compute(data: &[u8]) -> Self {
        Self(blake3::hash(data))
    }

    /// Compute Blake3 hash of data using multiple threads (for large data)
    pub fn compute_parallel(data: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update_rayon(data);
        Self(hasher.finalize())
    }

    /// Get the raw hash bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        self.0.to_hex().to_string()
    }

    /// Parse from hex string
    pub fn from_hex(hex: &str) -> Result<Self> {
        let hash = blake3::Hash::from_hex(hex)
            .map_err(|e| CyxCloudError::InvalidChunkId(e.to_string()))?;
        Ok(Self(hash))
    }

    /// Verify that data matches this hash
    pub fn verify(&self, data: &[u8]) -> bool {
        let computed = Self::compute(data);
        self == &computed
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.as_bytes())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Invalid hash length"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(blake3::Hash::from_bytes(arr)))
    }
}

/// AES-256-GCM encryption key
#[derive(Clone)]
pub struct EncryptionKey([u8; KEY_SIZE]);

impl EncryptionKey {
    /// Generate a new random encryption key
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    /// Create from raw bytes
    pub fn from_bytes(bytes: [u8; KEY_SIZE]) -> Self {
        Self(bytes)
    }

    /// Create from a slice (validates length)
    pub fn from_slice(slice: &[u8]) -> Result<Self> {
        if slice.len() != KEY_SIZE {
            return Err(CyxCloudError::InvalidKeyLength {
                expected: KEY_SIZE,
                actual: slice.len(),
            });
        }
        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(slice);
        Ok(Self(key))
    }

    /// Derive key from password using Argon2
    pub fn derive_from_password(password: &[u8], salt: &[u8]) -> Result<Self> {
        use argon2::password_hash::SaltString;
        use argon2::{Argon2, PasswordHasher};

        // Create salt string (must be base64-encoded)
        let salt_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, salt);
        let salt_string = SaltString::from_b64(&salt_b64)
            .map_err(|e| CyxCloudError::Encryption(e.to_string()))?;

        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password, &salt_string)
            .map_err(|e| CyxCloudError::Encryption(e.to_string()))?;

        let hash_bytes = password_hash
            .hash
            .ok_or_else(|| CyxCloudError::Encryption("No hash output".to_string()))?;

        Self::from_slice(hash_bytes.as_bytes())
    }

    /// Get the raw key bytes
    pub fn as_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.0
    }
}

impl fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EncryptionKey([REDACTED])")
    }
}

impl Drop for EncryptionKey {
    fn drop(&mut self) {
        // Zeroize key on drop for security
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

/// Encrypted data container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Nonce used for encryption (unique per encryption)
    pub nonce: [u8; NONCE_SIZE],
    /// Ciphertext with authentication tag appended
    pub ciphertext: Vec<u8>,
}

impl EncryptedData {
    /// Total overhead per encryption (nonce + tag)
    pub const OVERHEAD: usize = NONCE_SIZE + TAG_SIZE;

    /// Get the size of the encrypted data
    pub fn len(&self) -> usize {
        self.ciphertext.len() + NONCE_SIZE
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.ciphertext.is_empty()
    }

    /// Serialize to bytes (nonce prepended to ciphertext)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(NONCE_SIZE + self.ciphertext.len());
        result.extend_from_slice(&self.nonce);
        result.extend_from_slice(&self.ciphertext);
        result
    }

    /// Deserialize from bytes (nonce prepended to ciphertext)
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < NONCE_SIZE + TAG_SIZE {
            return Err(CyxCloudError::Decryption(
                "Data too short for encrypted content".to_string(),
            ));
        }

        let mut nonce = [0u8; NONCE_SIZE];
        nonce.copy_from_slice(&data[..NONCE_SIZE]);

        Ok(Self {
            nonce,
            ciphertext: data[NONCE_SIZE..].to_vec(),
        })
    }
}

/// Encrypt data using AES-256-GCM
pub fn encrypt(plaintext: &[u8], key: &EncryptionKey) -> Result<EncryptedData> {
    use rand::RngCore;

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Create cipher and encrypt
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CyxCloudError::Encryption(e.to_string()))?;

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CyxCloudError::Encryption(e.to_string()))?;

    Ok(EncryptedData {
        nonce: nonce_bytes,
        ciphertext,
    })
}

/// Decrypt data using AES-256-GCM
pub fn decrypt(encrypted: &EncryptedData, key: &EncryptionKey) -> Result<Vec<u8>> {
    let nonce = Nonce::from_slice(&encrypted.nonce);

    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CyxCloudError::Decryption(e.to_string()))?;

    let plaintext = cipher
        .decrypt(nonce, encrypted.ciphertext.as_slice())
        .map_err(|_| CyxCloudError::Decryption("Authentication failed".to_string()))?;

    Ok(plaintext)
}

/// Encrypt data and return as bytes (nonce prepended)
pub fn encrypt_to_bytes(plaintext: &[u8], key: &EncryptionKey) -> Result<Vec<u8>> {
    let encrypted = encrypt(plaintext, key)?;
    let mut result = Vec::with_capacity(NONCE_SIZE + encrypted.ciphertext.len());
    result.extend_from_slice(&encrypted.nonce);
    result.extend_from_slice(&encrypted.ciphertext);
    Ok(result)
}

/// Decrypt data from bytes (nonce prepended)
pub fn decrypt_from_bytes(data: &[u8], key: &EncryptionKey) -> Result<Vec<u8>> {
    if data.len() < NONCE_SIZE + TAG_SIZE {
        return Err(CyxCloudError::Decryption(
            "Data too short for encrypted content".to_string(),
        ));
    }

    let mut nonce = [0u8; NONCE_SIZE];
    nonce.copy_from_slice(&data[..NONCE_SIZE]);

    let encrypted = EncryptedData {
        nonce,
        ciphertext: data[NONCE_SIZE..].to_vec(),
    };

    decrypt(&encrypted, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash() {
        let data = b"hello world";
        let hash = ContentHash::compute(data);

        // Same data produces same hash
        let hash2 = ContentHash::compute(data);
        assert_eq!(hash, hash2);

        // Different data produces different hash
        let hash3 = ContentHash::compute(b"different data");
        assert_ne!(hash, hash3);

        // Verification works
        assert!(hash.verify(data));
        assert!(!hash.verify(b"wrong data"));
    }

    #[test]
    fn test_content_hash_parallel() {
        let data = vec![0u8; 1024 * 1024]; // 1 MB
        let hash1 = ContentHash::compute(&data);
        let hash2 = ContentHash::compute_parallel(&data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_encryption_roundtrip() {
        let key = EncryptionKey::generate();
        let plaintext = b"secret message";

        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_encryption_to_bytes_roundtrip() {
        let key = EncryptionKey::generate();
        let plaintext = b"another secret";

        let encrypted = encrypt_to_bytes(plaintext, &key).unwrap();
        let decrypted = decrypt_from_bytes(&encrypted, &key).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_wrong_key_fails() {
        let key1 = EncryptionKey::generate();
        let key2 = EncryptionKey::generate();
        let plaintext = b"secret";

        let encrypted = encrypt(plaintext, &key1).unwrap();
        let result = decrypt(&encrypted, &key2);

        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let key = EncryptionKey::generate();
        let plaintext = b"secret";

        let mut encrypted = encrypt(plaintext, &key).unwrap();
        // Tamper with ciphertext
        if let Some(byte) = encrypted.ciphertext.get_mut(0) {
            *byte ^= 0xFF;
        }

        let result = decrypt(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encryption_overhead() {
        let key = EncryptionKey::generate();
        let plaintext = vec![0u8; 1000];

        let encrypted = encrypt(&plaintext, &key).unwrap();
        // Ciphertext should be plaintext + 16 byte auth tag
        assert_eq!(encrypted.ciphertext.len(), plaintext.len() + TAG_SIZE);
    }
}
