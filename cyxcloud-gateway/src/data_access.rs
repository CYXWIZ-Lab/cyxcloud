//! Data Access Token Service
//!
//! Manages short-lived tokens for Server Nodes to access dataset data directly.
//! Tokens are:
//! - Ed25519 signed by the gateway
//! - Scoped to specific dataset + node + permissions
//! - Time-limited (default 24 hours)
//! - Revocable

use crate::AppState;
use chrono::{DateTime, Duration, Utc};
use cyxcloud_metadata::CreateDataAccessToken;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// Data access token errors
#[derive(Error, Debug)]
pub enum DataAccessError {
    #[error("Dataset not found: {0}")]
    DatasetNotFound(Uuid),

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Token not found")]
    TokenNotFound,

    #[error("Token expired")]
    TokenExpired,

    #[error("Token revoked")]
    TokenRevoked,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Database error: {0}")]
    Database(String),
}

pub type DataAccessResult<T> = Result<T, DataAccessError>;

/// Default token TTL in seconds (24 hours)
pub const DEFAULT_TOKEN_TTL_SECS: i64 = 24 * 60 * 60;

/// Maximum token TTL in seconds (7 days)
pub const MAX_TOKEN_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Token data that gets signed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    /// Unique token ID
    pub token_id: Uuid,

    /// Dataset this token grants access to
    pub dataset_id: Uuid,

    /// Node this token is restricted to (optional)
    pub node_id: Option<Uuid>,

    /// User who created the token
    pub user_id: Uuid,

    /// Permissions granted: ["read", "stream"]
    pub scopes: Vec<String>,

    /// Expiration timestamp
    pub expires_at: i64,

    /// Creation timestamp
    pub created_at: i64,

    /// Random nonce for uniqueness
    pub nonce: [u8; 16],
}

/// Complete signed token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedToken {
    /// Token data
    pub data: TokenData,

    /// Ed25519 signature of the token data
    pub signature: Vec<u8>,
}

impl SignedToken {
    /// Encode token to base64 string for transmission
    pub fn encode(&self) -> Result<String, DataAccessError> {
        let json = serde_json::to_vec(self).map_err(|_| DataAccessError::InvalidToken)?;
        Ok(base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &json,
        ))
    }

    /// Decode token from base64 string
    pub fn decode(token_str: &str) -> Result<Self, DataAccessError> {
        let json = base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            token_str,
        )
        .map_err(|_| DataAccessError::InvalidToken)?;

        serde_json::from_slice(&json).map_err(|_| DataAccessError::InvalidToken)
    }
}

/// Request to create a new access token
#[derive(Debug, Clone)]
pub struct CreateTokenRequest {
    pub dataset_id: Uuid,
    pub node_id: Option<Uuid>,
    pub user_id: Uuid,
    pub scopes: Vec<String>,
    pub ttl_seconds: Option<i64>,
}

/// Response after creating a token
#[derive(Debug, Clone)]
pub struct TokenResponse {
    /// The encoded token string
    pub token: String,

    /// Token ID for revocation
    pub token_id: Uuid,

    /// Expiration timestamp
    pub expires_at: DateTime<Utc>,

    /// Granted scopes
    pub scopes: Vec<String>,
}

/// Data Access Token Service
pub struct DataAccessTokenService {
    state: Arc<AppState>,
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl DataAccessTokenService {
    /// Create a new service with a generated signing key
    pub fn new(state: Arc<AppState>) -> Self {
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();

        Self {
            state,
            signing_key,
            verifying_key,
        }
    }

    /// Create a new service with a provided signing key
    pub fn with_key(state: Arc<AppState>, signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();

        Self {
            state,
            signing_key,
            verifying_key,
        }
    }

    /// Create a new access token
    #[instrument(skip(self), fields(dataset_id = %request.dataset_id, user_id = %request.user_id))]
    pub async fn create_token(
        &self,
        request: CreateTokenRequest,
    ) -> DataAccessResult<TokenResponse> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| DataAccessError::Database("Metadata service not available".into()))?;

        // Verify dataset exists
        let dataset = metadata
            .database()
            .get_dataset(request.dataset_id)
            .await
            .map_err(|e| DataAccessError::Database(e.to_string()))?
            .ok_or(DataAccessError::DatasetNotFound(request.dataset_id))?;

        // Verify user has access to dataset
        if dataset.owner_id != request.user_id {
            // Check if shared with user
            let has_access = metadata
                .database()
                .check_dataset_access(request.dataset_id, request.user_id)
                .await
                .map_err(|e| DataAccessError::Database(e.to_string()))?;

            if has_access.is_none() {
                return Err(DataAccessError::PermissionDenied);
            }
        }

        // Calculate TTL
        let ttl_secs = request
            .ttl_seconds
            .unwrap_or(DEFAULT_TOKEN_TTL_SECS)
            .min(MAX_TOKEN_TTL_SECS);

        let now = Utc::now();
        let expires_at = now + Duration::seconds(ttl_secs);

        // Generate token ID and nonce
        let token_id = Uuid::new_v4();
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        // Create token data
        let token_data = TokenData {
            token_id,
            dataset_id: request.dataset_id,
            node_id: request.node_id,
            user_id: request.user_id,
            scopes: request.scopes.clone(),
            expires_at: expires_at.timestamp(),
            created_at: now.timestamp(),
            nonce,
        };

        // Serialize and sign
        let token_bytes =
            serde_json::to_vec(&token_data).map_err(|_| DataAccessError::InvalidToken)?;
        let signature = self.signing_key.sign(&token_bytes);

        // Create signed token
        let signed_token = SignedToken {
            data: token_data,
            signature: signature.to_bytes().to_vec(),
        };

        // Encode token
        let token_string = signed_token.encode()?;

        // Store token hash in database for revocation checking
        let token_hash = blake3::hash(token_string.as_bytes()).as_bytes().to_vec();

        let create_token = CreateDataAccessToken {
            id: token_id,
            dataset_id: request.dataset_id,
            node_id: request.node_id,
            user_id: request.user_id,
            token_hash,
            scopes: request.scopes.clone(),
            expires_at,
        };

        metadata
            .database()
            .create_data_access_token(create_token)
            .await
            .map_err(|e| DataAccessError::Database(e.to_string()))?;

        info!(
            token_id = %token_id,
            dataset_id = %request.dataset_id,
            ttl_secs = ttl_secs,
            "Created data access token"
        );

        Ok(TokenResponse {
            token: token_string,
            token_id,
            expires_at,
            scopes: request.scopes,
        })
    }

    /// Verify a token and return the token data
    #[instrument(skip(self, token_string))]
    pub async fn verify_token(&self, token_string: &str) -> DataAccessResult<TokenData> {
        // Decode token
        let signed_token = SignedToken::decode(token_string)?;

        // Verify signature
        let token_bytes = serde_json::to_vec(&signed_token.data)
            .map_err(|_| DataAccessError::InvalidToken)?;

        let sig_array: [u8; 64] = signed_token
            .signature
            .try_into()
            .map_err(|_| DataAccessError::InvalidSignature)?;
        let signature = Signature::from_bytes(&sig_array);

        self.verifying_key
            .verify(&token_bytes, &signature)
            .map_err(|_| DataAccessError::InvalidSignature)?;

        // Check expiration
        let now = Utc::now().timestamp();
        if signed_token.data.expires_at < now {
            return Err(DataAccessError::TokenExpired);
        }

        // Check if revoked (in database)
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| DataAccessError::Database("Metadata service not available".into()))?;

        let stored_token = metadata
            .database()
            .get_data_access_token(signed_token.data.token_id)
            .await
            .map_err(|e| DataAccessError::Database(e.to_string()))?;

        match stored_token {
            Some(token) if token.revoked_at.is_some() => {
                return Err(DataAccessError::TokenRevoked);
            }
            None => {
                // Token not in database - might be revoked or never stored
                warn!(token_id = %signed_token.data.token_id, "Token not found in database");
                return Err(DataAccessError::TokenNotFound);
            }
            _ => {}
        }

        debug!(token_id = %signed_token.data.token_id, "Token verified successfully");
        Ok(signed_token.data)
    }

    /// Revoke a token by ID
    #[instrument(skip(self), fields(token_id = %token_id))]
    pub async fn revoke_token(&self, token_id: Uuid, user_id: Uuid) -> DataAccessResult<()> {
        let metadata = self
            .state
            .metadata_service()
            .ok_or_else(|| DataAccessError::Database("Metadata service not available".into()))?;

        // Get token to verify ownership
        let token = metadata
            .database()
            .get_data_access_token(token_id)
            .await
            .map_err(|e| DataAccessError::Database(e.to_string()))?
            .ok_or(DataAccessError::TokenNotFound)?;

        // Verify user owns this token
        if token.user_id != user_id {
            return Err(DataAccessError::PermissionDenied);
        }

        // Revoke
        metadata
            .database()
            .revoke_data_access_token(token_id)
            .await
            .map_err(|e| DataAccessError::Database(e.to_string()))?;

        info!(token_id = %token_id, "Token revoked");
        Ok(())
    }

    /// Check if a token has a specific scope
    pub fn has_scope(token_data: &TokenData, scope: &str) -> bool {
        token_data.scopes.iter().any(|s| s == scope || s == "*")
    }

    /// Get the verifying key (public key) for external verification
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }
}

/// Standard scopes for data access
pub mod scopes {
    /// Read dataset metadata
    pub const READ: &str = "read";

    /// Stream dataset batches
    pub const STREAM: &str = "stream";

    /// Full access
    pub const ALL: &str = "*";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_encode_decode() {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let token_data = TokenData {
            token_id: Uuid::new_v4(),
            dataset_id: Uuid::new_v4(),
            node_id: Some(Uuid::new_v4()),
            user_id: Uuid::new_v4(),
            scopes: vec!["read".to_string(), "stream".to_string()],
            expires_at: Utc::now().timestamp() + 3600,
            created_at: Utc::now().timestamp(),
            nonce,
        };

        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let token_bytes = serde_json::to_vec(&token_data).unwrap();
        let signature = signing_key.sign(&token_bytes);

        let signed_token = SignedToken {
            data: token_data.clone(),
            signature: signature.to_bytes().to_vec(),
        };

        // Encode
        let encoded = signed_token.encode().unwrap();
        assert!(!encoded.is_empty());

        // Decode
        let decoded = SignedToken::decode(&encoded).unwrap();
        assert_eq!(decoded.data.token_id, token_data.token_id);
        assert_eq!(decoded.data.dataset_id, token_data.dataset_id);
        assert_eq!(decoded.data.scopes, token_data.scopes);
    }

    #[test]
    fn test_has_scope() {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let token_data = TokenData {
            token_id: Uuid::new_v4(),
            dataset_id: Uuid::new_v4(),
            node_id: None,
            user_id: Uuid::new_v4(),
            scopes: vec!["read".to_string(), "stream".to_string()],
            expires_at: 0,
            created_at: 0,
            nonce,
        };

        assert!(DataAccessTokenService::has_scope(&token_data, "read"));
        assert!(DataAccessTokenService::has_scope(&token_data, "stream"));
        assert!(!DataAccessTokenService::has_scope(&token_data, "write"));
    }

    #[test]
    fn test_wildcard_scope() {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let token_data = TokenData {
            token_id: Uuid::new_v4(),
            dataset_id: Uuid::new_v4(),
            node_id: None,
            user_id: Uuid::new_v4(),
            scopes: vec!["*".to_string()],
            expires_at: 0,
            created_at: 0,
            nonce,
        };

        assert!(DataAccessTokenService::has_scope(&token_data, "read"));
        assert!(DataAccessTokenService::has_scope(&token_data, "stream"));
        assert!(DataAccessTokenService::has_scope(&token_data, "anything"));
    }
}
