//! Authentication module for CyxCloud Gateway
//!
//! Provides:
//! - JWT token generation and validation
//! - Wallet signature verification (Solana/Ed25519)
//! - API key management
//! - Authentication middleware

#![allow(unused_imports)]

use chrono::{Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, TokenData, Validation};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Authentication errors
#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Token expired")]
    TokenExpired,

    #[error("Token has been revoked")]
    Revoked,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Invalid wallet address")]
    InvalidWalletAddress,

    #[error("User not found")]
    UserNotFound,

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type AuthResult<T> = Result<T, AuthError>;

/// JWT claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,

    /// Expiration time (Unix timestamp)
    pub exp: i64,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Not before (Unix timestamp); absent in CyxWiz API sign-in tokens.
    #[serde(default)]
    pub nbf: i64,

    /// JWT ID (unique identifier); CyxWiz API sign-in tokens have none, so
    /// they cannot be revoked one by one (they expire within 24 h).
    #[serde(default)]
    pub jti: String,

    /// User type (user, node, api_key); CyxWiz API sign-in tokens are users.
    #[serde(default = "default_user_type")]
    pub user_type: String,

    /// Wallet address (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet: Option<String>,

    /// Permissions
    #[serde(default)]
    pub permissions: Vec<String>,

    /// What the token is for (`TokenType::claim`): access, refresh, node,
    /// api_key. Every token CyxCloud issues has it; CyxWiz API sign-in tokens
    /// (no `jti`) have none and count as access tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,

    /// Issuer. Gateway tokens have none; CyxWiz API sign-in tokens carry
    /// `API_TOKEN_ISSUER`, which is the only jti-less token accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
}

/// The issuer of CyxWiz API sign-in tokens (and the website's 5-minute API
/// tokens): the only tokens without a jti the gateway accepts, whatever the
/// issuer settings (central's P2P tokens share the secret in some setups).
pub const API_TOKEN_ISSUER: &str = "cyxwiz-api";

fn default_user_type() -> String {
    "user".to_string()
}

/// Namespace for user ids derived from CyxWiz API account ids (UUID v5).
/// Fixed for ever: changing it would hand every account new, empty storage.
pub const CYXWIZ_USER_NAMESPACE: Uuid = Uuid::from_u128(0x7c1e_4f2a_9b3d_4e6f_8a51_c2d3_e4f5_0617);

/// The CyxCloud user id behind a token subject. CyxCloud's own tokens carry a
/// UUID; a CyxWiz API sign-in token carries the API's user id (a Mongo
/// ObjectId), which maps to one stable UUID so the same account owns the same
/// datasets whether it arrives from the website, the Engine or a node
/// (TOFIX164).
pub fn user_uuid_for_subject(sub: &str) -> Uuid {
    Uuid::parse_str(sub).unwrap_or_else(|_| Uuid::new_v5(&CYXWIZ_USER_NAMESPACE, sub.as_bytes()))
}

impl Claims {
    /// The CyxCloud user id of this token's subject (see user_uuid_for_subject).
    pub fn user_uuid(&self) -> Uuid {
        user_uuid_for_subject(&self.sub)
    }
}

/// Token type for different authentication flows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    /// Short-lived access token (1 hour)
    Access,
    /// Long-lived refresh token (7 days)
    Refresh,
    /// Node authentication token (24 hours)
    Node,
    /// API key token (no expiration by default)
    ApiKey,
}

impl TokenType {
    /// The `typ` claim of a token of this type.
    pub fn claim(&self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
            Self::Node => "node",
            Self::ApiKey => "api_key",
        }
    }

    /// Get token lifetime in seconds
    pub fn lifetime_secs(&self) -> i64 {
        match self {
            Self::Access => 3600,            // 1 hour
            Self::Refresh => 7 * 24 * 3600,  // 7 days
            Self::Node => 24 * 3600,         // 24 hours
            Self::ApiKey => 365 * 24 * 3600, // 1 year (effectively no expiration)
        }
    }
}

/// Configuration for the auth service
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Secret key for JWT signing (32 bytes minimum)
    pub jwt_secret: Vec<u8>,

    /// Issuer claim for JWTs (can be multiple, comma-separated)
    pub issuer: String,

    /// Additional issuers to accept (e.g., from CyxWiz API)
    pub accepted_issuers: Vec<String>,

    /// Audience claim for JWTs
    pub audience: String,

    /// Whether to require wallet verification
    pub require_wallet_verification: bool,

    /// Skip issuer validation (for cross-service token sharing)
    pub skip_issuer_validation: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            // Generate a random secret for development
            // In production, this should come from environment/config
            jwt_secret: rand::random::<[u8; 32]>().to_vec(),
            issuer: "cyxcloud-gateway".to_string(),
            accepted_issuers: vec!["cyxwiz-api".to_string()],
            audience: "cyxcloud".to_string(),
            require_wallet_verification: false,
            skip_issuer_validation: false, // Issuer validation enabled by default
        }
    }
}

impl AuthConfig {
    /// Create config from environment variables
    ///
    /// In production (CYXCLOUD_ENV=production), JWT_SECRET is required.
    /// In development, a random secret is generated if not set.
    pub fn from_env() -> Self {
        let is_production = std::env::var("CYXCLOUD_ENV")
            .map(|v| v.to_lowercase() == "production")
            .unwrap_or(false);

        let jwt_secret = std::env::var("JWT_SECRET")
            .map(|s| s.into_bytes())
            .unwrap_or_else(|_| {
                if is_production {
                    panic!(
                        "JWT_SECRET environment variable is required in production. \
                         Set CYXCLOUD_ENV=development to use a random secret for testing."
                    );
                }
                warn!("JWT_SECRET not set, using random secret (not suitable for production)");
                rand::random::<[u8; 32]>().to_vec()
            });

        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "cyxcloud-gateway".to_string());

        // Accept multiple issuers (comma-separated)
        let accepted_issuers = std::env::var("JWT_ACCEPTED_ISSUERS")
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_else(|_| vec!["cyxwiz-api".to_string()]);

        let audience = std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "cyxcloud".to_string());

        let require_wallet = std::env::var("REQUIRE_WALLET_VERIFICATION")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        // Default to false: issuer validation is enabled by default for security.
        // Set JWT_SKIP_ISSUER_VALIDATION=true only if you need cross-service token sharing
        // without issuer checks (not recommended for production).
        let skip_issuer = std::env::var("JWT_SKIP_ISSUER_VALIDATION")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        Self {
            jwt_secret,
            issuer,
            accepted_issuers,
            audience,
            require_wallet_verification: require_wallet,
            skip_issuer_validation: skip_issuer,
        }
    }
}

/// Authentication service
pub struct AuthService {
    config: AuthConfig,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    validation: Validation,
    /// Local cache of revoked token IDs (JTIs) — acts as L1 cache
    /// Revoked token ids and their expiry (Unix seconds); an entry is
    /// dropped once its token would have expired anyway.
    revoked_tokens: RwLock<std::collections::HashMap<String, i64>>,
    /// Redis connection for persistent revocation (L2) — survives restarts
    redis: Option<RwLock<redis::aio::MultiplexedConnection>>,
}

impl AuthService {
    /// Create a new auth service with the given config
    pub fn new(config: AuthConfig) -> Self {
        let encoding_key = EncodingKey::from_secret(&config.jwt_secret);
        let decoding_key = DecodingKey::from_secret(&config.jwt_secret);

        let mut validation = Validation::default();

        // Configure issuer validation
        if config.skip_issuer_validation {
            // Skip issuer validation to accept tokens from CyxWiz API
            // Don't set issuer - validation will skip issuer check
            validation.set_required_spec_claims::<&str>(&["exp", "sub"]);
            info!("JWT issuer validation disabled - accepting tokens from external services");
        } else {
            // Accept tokens from gateway and additional issuers (e.g., CyxWiz API)
            let mut all_issuers = vec![config.issuer.clone()];
            all_issuers.extend(config.accepted_issuers.clone());
            validation.set_issuer(&all_issuers.iter().map(|s| s.as_str()).collect::<Vec<_>>());
            info!(issuers = ?all_issuers, "JWT issuer validation enabled");
        }

        // Skip audience validation for cross-service compatibility
        validation.set_audience::<&str>(&[]);

        Self {
            config,
            encoding_key,
            decoding_key,
            validation,
            revoked_tokens: RwLock::new(std::collections::HashMap::new()),
            redis: None,
        }
    }

    /// Create auth service from environment
    pub fn from_env() -> Self {
        Self::new(AuthConfig::from_env())
    }

    /// Attach a Redis connection for persistent token revocation.
    /// Without Redis, revocations are in-memory only and lost on restart.
    pub fn with_redis(mut self, conn: redis::aio::MultiplexedConnection) -> Self {
        self.redis = Some(RwLock::new(conn));
        info!("Token revocation backed by Redis");
        self
    }

    /// Generate a JWT token for a user, with its type's lifetime
    pub fn generate_token(
        &self,
        user_id: &str,
        token_type: TokenType,
        wallet: Option<String>,
        permissions: Vec<String>,
    ) -> AuthResult<String> {
        let lifetime = token_type.lifetime_secs();
        self.generate_token_for(user_id, token_type, wallet, permissions, lifetime)
    }

    /// Generate a JWT token that lives `lifetime_secs` (an API key's chosen
    /// expiry).
    pub fn generate_token_for(
        &self,
        user_id: &str,
        token_type: TokenType,
        wallet: Option<String>,
        permissions: Vec<String>,
        lifetime_secs: i64,
    ) -> AuthResult<String> {
        let now = Utc::now();
        let exp = now + Duration::seconds(lifetime_secs);

        let user_type = match token_type {
            TokenType::Access | TokenType::Refresh => "user",
            TokenType::Node => "node",
            TokenType::ApiKey => "api_key",
        };

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            nbf: now.timestamp(),
            jti: Uuid::new_v4().to_string(),
            user_type: user_type.to_string(),
            wallet,
            permissions,
            typ: Some(token_type.claim().to_string()),
            iss: None,
        };

        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| AuthError::Internal(format!("Failed to generate token: {}", e)))
    }

    /// Validate a token presented to use the service (access, node or API key
    /// token, or a CyxWiz API sign-in token) and return its claims. A
    /// refresh token is refused here: it is only good at
    /// /api/v1/auth/refresh (`validate_refresh_token`). A CyxCloud token
    /// without a `typ` (issued before token types) is refused too.
    pub async fn validate_token(&self, token: &str) -> AuthResult<Claims> {
        let claims = self.decode_unrevoked(token).await?;
        if claims.jti.is_empty() {
            // A CyxWiz API sign-in token: an access token by definition.
            // Nothing else without a jti is accepted, even with issuer
            // checking off.
            if claims.iss.as_deref() != Some(API_TOKEN_ISSUER) {
                return Err(AuthError::InvalidToken(
                    "not a CyxWiz API sign-in token".to_string(),
                ));
            }
            return Ok(claims);
        }
        match claims.typ.as_deref() {
            Some("access") | Some("node") | Some("api_key") => Ok(claims),
            Some("refresh") => Err(AuthError::InvalidToken(
                "a refresh token cannot be used for access".to_string(),
            )),
            _ => Err(AuthError::InvalidToken(
                "token has no type; sign in again".to_string(),
            )),
        }
    }

    /// Validate a refresh token (only at /api/v1/auth/refresh).
    pub async fn validate_refresh_token(&self, token: &str) -> AuthResult<Claims> {
        let claims = self.decode_unrevoked(token).await?;
        if claims.typ.as_deref() != Some("refresh") {
            return Err(AuthError::InvalidToken("not a refresh token".to_string()));
        }
        Ok(claims)
    }

    /// Decode a token (signature, expiry, issuer) and check it is not revoked.
    ///
    /// Checks revocation in two tiers:
    /// 1. L1: local in-memory HashSet (fast, but lost on restart)
    /// 2. L2: Redis (persistent, survives restarts)
    async fn decode_unrevoked(&self, token: &str) -> AuthResult<Claims> {
        // Decode and validate token
        let token_data: TokenData<Claims> = decode(token, &self.decoding_key, &self.validation)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
                _ => AuthError::InvalidToken(e.to_string()),
            })?;

        let jti = &token_data.claims.jti;
        if jti.is_empty() {
            // A CyxWiz API sign-in token: nothing to look up for revocation.
            return Ok(token_data.claims);
        }

        // L1: check local cache
        {
            let revoked = self.revoked_tokens.read().await;
            if revoked.contains_key(jti) {
                return Err(AuthError::Revoked);
            }
        }

        // L2: check Redis (if available)
        if let Some(ref redis) = self.redis {
            let key = format!("revoked:{}", jti);
            let mut conn = redis.write().await;
            match conn.exists::<_, bool>(&key).await {
                Ok(true) => {
                    // Populate L1 cache so subsequent checks are fast
                    let mut revoked = self.revoked_tokens.write().await;
                    revoked.insert(jti.clone(), token_data.claims.exp);
                    return Err(AuthError::Revoked);
                }
                Ok(false) => {} // Not revoked
                Err(e) => {
                    // Fail-open: if Redis is down, allow the request but warn
                    warn!(jti = %jti, error = %e, "Redis revocation check failed (fail-open)");
                }
            }
        }

        Ok(token_data.claims)
    }

    /// Revoke a token by its JTI until its expiry `exp` (Unix seconds), so
    /// a revoked long-lived token (an API key) stays revoked for its life.
    ///
    /// Stores in both the local in-memory cache (L1) and Redis (L2) if
    /// available; the Redis entry expires with the token.
    pub async fn revoke_token(&self, jti: &str, exp: i64) {
        // L1: local cache
        {
            let mut revoked = self.revoked_tokens.write().await;
            let now = Utc::now().timestamp();
            revoked.retain(|_, until| *until > now);
            revoked.insert(jti.to_string(), exp);
        }

        // L2: Redis (persistent across restarts)
        if let Some(ref redis) = self.redis {
            let key = format!("revoked:{}", jti);
            // Kept until the token would have expired anyway.
            let ttl_secs = (exp - Utc::now().timestamp()).max(1) as u64;
            let mut conn = redis.write().await;
            if let Err(e) = conn.set_ex::<_, _, ()>(&key, "1", ttl_secs).await {
                warn!(jti = %jti, error = %e, "Failed to persist token revocation to Redis");
            } else {
                debug!(jti = %jti, ttl_secs = ttl_secs, "Token revocation persisted to Redis");
            }
        }
    }

    /// Verify a Solana wallet signature
    ///
    /// The message should be a challenge string that was signed by the wallet.
    /// The signature should be base58-encoded.
    pub fn verify_wallet_signature(
        &self,
        wallet_address: &str,
        message: &[u8],
        signature_b58: &str,
    ) -> AuthResult<bool> {
        // Decode the wallet address (base58 public key)
        let pubkey_bytes = bs58::decode(wallet_address)
            .into_vec()
            .map_err(|_| AuthError::InvalidWalletAddress)?;

        if pubkey_bytes.len() != 32 {
            return Err(AuthError::InvalidWalletAddress);
        }

        // Parse the public key
        let pubkey_array: [u8; 32] = pubkey_bytes
            .try_into()
            .map_err(|_| AuthError::InvalidWalletAddress)?;

        let verifying_key =
            VerifyingKey::from_bytes(&pubkey_array).map_err(|_| AuthError::InvalidWalletAddress)?;

        // Decode the signature (base58)
        let sig_bytes = bs58::decode(signature_b58)
            .into_vec()
            .map_err(|_| AuthError::InvalidSignature)?;

        if sig_bytes.len() != 64 {
            return Err(AuthError::InvalidSignature);
        }

        let sig_array: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| AuthError::InvalidSignature)?;

        let signature = Signature::from_bytes(&sig_array);

        // Verify the signature
        verifying_key
            .verify(message, &signature)
            .map_err(|_| AuthError::InvalidSignature)?;

        Ok(true)
    }

    /// Generate a challenge message for wallet authentication
    pub fn generate_challenge(&self) -> (String, String) {
        let nonce = Uuid::new_v4().to_string();
        let timestamp = Utc::now().timestamp();
        let message = format!(
            "Sign this message to authenticate with CyxCloud.\n\nNonce: {}\nTimestamp: {}",
            nonce, timestamp
        );
        (nonce, message)
    }

    /// Check if claims have a specific permission
    pub fn has_permission(claims: &Claims, permission: &str) -> bool {
        claims.permissions.contains(&permission.to_string())
            || claims.permissions.contains(&"*".to_string())
    }

    /// Check if claims have any of the specified permissions
    pub fn has_any_permission(claims: &Claims, permissions: &[&str]) -> bool {
        permissions.iter().any(|p| Self::has_permission(claims, p))
    }
}

/// Request body for wallet login
#[derive(Debug, Deserialize)]
pub struct WalletLoginRequest {
    /// Wallet address (base58 public key)
    pub wallet_address: String,

    /// Challenge message that was signed
    pub message: String,

    /// Signature (base58)
    pub signature: String,
}

/// Request body for API key creation
#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    /// Name for the API key
    pub name: String,

    /// Permissions to grant
    #[serde(default)]
    pub permissions: Vec<String>,

    /// Expiration in days (optional, default 365)
    pub expires_in_days: Option<i64>,
}

/// Response for authentication endpoints
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    /// Access token
    pub access_token: String,

    /// Refresh token (for user auth)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,

    /// Token type (always "Bearer")
    pub token_type: String,

    /// Expiration time in seconds
    pub expires_in: i64,

    /// User ID
    pub user_id: String,
}

/// Response for challenge generation
#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    /// Nonce (should be stored temporarily)
    pub nonce: String,

    /// Message to sign
    pub message: String,

    /// Challenge expires at (Unix timestamp)
    pub expires_at: i64,
}

/// User info from token
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub user_type: String,
    pub wallet: Option<String>,
    pub permissions: Vec<String>,
}

impl From<Claims> for AuthUser {
    fn from(claims: Claims) -> Self {
        Self {
            id: claims.sub,
            user_type: claims.user_type,
            wallet: claims.wallet,
            permissions: claims.permissions,
        }
    }
}

/// Standard permissions
pub mod permissions {
    pub const STORAGE_READ: &str = "storage:read";
    pub const STORAGE_WRITE: &str = "storage:write";
    pub const STORAGE_DELETE: &str = "storage:delete";
    pub const DATASET_READ: &str = "dataset:read";
    pub const DATASET_WRITE: &str = "dataset:write";
    pub const NODE_REGISTER: &str = "node:register";
    pub const NODE_ADMIN: &str = "node:admin";
    pub const ADMIN: &str = "*";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_validate_token() {
        let auth = AuthService::new(AuthConfig::default());

        let token = auth
            .generate_token(
                "user-123",
                TokenType::Access,
                Some("wallet-address".to_string()),
                vec!["storage:read".to_string()],
            )
            .unwrap();

        assert!(!token.is_empty());

        // Validate in async context
        let rt = tokio::runtime::Runtime::new().unwrap();
        let claims = rt.block_on(auth.validate_token(&token)).unwrap();

        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.wallet, Some("wallet-address".to_string()));
        assert!(claims.permissions.contains(&"storage:read".to_string()));
    }

    #[test]
    fn test_generate_challenge() {
        let auth = AuthService::new(AuthConfig::default());
        let (nonce, message) = auth.generate_challenge();

        assert!(!nonce.is_empty());
        assert!(message.contains(&nonce));
        assert!(message.contains("CyxCloud"));
    }

    #[tokio::test]
    async fn accepts_cyxwiz_api_sign_in_tokens() {
        // The CyxWiz web API signs {sub, email, role, iat, exp, iss} with the
        // shared JWT_SECRET (TOFIX136 E2e).
        let config = AuthConfig::default();
        let secret = config.jwt_secret.clone();
        let auth = AuthService::new(config);
        let now = chrono::Utc::now().timestamp();
        let api_claims = serde_json::json!({
            "sub": "6ac29f910171abdbcfdf8a04", "email": "e2e-user@local.test", "role": "user",
            "iat": now, "exp": now + 3600, "iss": "cyxwiz-api"
        });
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &api_claims,
            &jsonwebtoken::EncodingKey::from_secret(&secret),
        )
        .unwrap();
        let claims = auth
            .validate_token(&token)
            .await
            .expect("API token accepted");
        assert_eq!(claims.sub, "6ac29f910171abdbcfdf8a04");
        assert_eq!(claims.user_type, "user");

        let expired = serde_json::json!({
            "sub": "x", "iat": now - 7200, "exp": now - 3600, "iss": "cyxwiz-api"
        });
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &expired,
            &jsonwebtoken::EncodingKey::from_secret(&secret),
        )
        .unwrap();
        assert!(
            auth.validate_token(&token).await.is_err(),
            "expired API token refused"
        );
    }

    #[tokio::test]
    async fn a_refresh_token_is_only_good_for_refreshing() {
        let config = AuthConfig::default();
        let secret = config.jwt_secret.clone();
        let auth = AuthService::new(config);
        let refresh = auth
            .generate_token("user-1", TokenType::Refresh, None, vec![])
            .unwrap();
        let access = auth
            .generate_token("user-1", TokenType::Access, None, vec![])
            .unwrap();

        assert!(
            auth.validate_token(&refresh).await.is_err(),
            "refresh refused for access"
        );
        assert_eq!(
            auth.validate_refresh_token(&refresh).await.unwrap().sub,
            "user-1"
        );
        assert!(
            auth.validate_refresh_token(&access).await.is_err(),
            "access refused for refresh"
        );
        assert!(auth.validate_token(&access).await.is_ok());

        // A CyxCloud token from before token types (a jti, no typ) is refused.
        let now = chrono::Utc::now().timestamp();
        let old = serde_json::json!({
            "sub": "user-1", "iat": now, "nbf": now, "exp": now + 3600,
            "jti": "old-token", "user_type": "user", "permissions": []
        });
        let old = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &old,
            &jsonwebtoken::EncodingKey::from_secret(&secret),
        )
        .unwrap();
        assert!(auth.validate_token(&old).await.is_err());
        assert!(auth.validate_refresh_token(&old).await.is_err());
    }

    #[tokio::test]
    async fn only_api_sign_in_tokens_may_lack_a_jti() {
        // Even with issuer checking off, a jti-less token must come from the
        // CyxWiz API (central's P2P tokens share the secret in some setups).
        let config = AuthConfig {
            skip_issuer_validation: true,
            ..AuthConfig::default()
        };
        let secret = config.jwt_secret.clone();
        let auth = AuthService::new(config);
        let now = chrono::Utc::now().timestamp();
        let sign = |iss: &str| {
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &serde_json::json!({ "sub": "u1", "iat": now, "exp": now + 600, "iss": iss }),
                &jsonwebtoken::EncodingKey::from_secret(&secret),
            )
            .unwrap()
        };
        assert!(auth.validate_token(&sign("cyxwiz-api")).await.is_ok());
        assert!(auth
            .validate_token(&sign("CyxWiz-Central-Server"))
            .await
            .is_err());
    }

    #[test]
    fn test_has_permission() {
        let claims = Claims {
            sub: "user-123".to_string(),
            exp: 0,
            iat: 0,
            nbf: 0,
            jti: "jti".to_string(),
            user_type: "user".to_string(),
            wallet: None,
            permissions: vec!["storage:read".to_string(), "storage:write".to_string()],
            typ: Some("access".to_string()),
            iss: None,
        };

        assert!(AuthService::has_permission(&claims, "storage:read"));
        assert!(AuthService::has_permission(&claims, "storage:write"));
        assert!(!AuthService::has_permission(&claims, "storage:delete"));
    }

    #[test]
    fn test_admin_permission() {
        let claims = Claims {
            sub: "admin".to_string(),
            exp: 0,
            iat: 0,
            nbf: 0,
            jti: "jti".to_string(),
            user_type: "user".to_string(),
            wallet: None,
            permissions: vec!["*".to_string()],
            typ: Some("access".to_string()),
            iss: None,
        };

        assert!(AuthService::has_permission(&claims, "storage:read"));
        assert!(AuthService::has_permission(&claims, "anything"));
    }

    #[test]
    fn api_subjects_map_to_one_stable_uuid_and_uuids_pass_through() {
        let a = user_uuid_for_subject("6ac29f910171abdbcfdf8a04");
        let b = user_uuid_for_subject("6ac29f910171abdbcfdf8a04");
        let other = user_uuid_for_subject("6ac29f910171abdbcfdf8a05");
        assert_eq!(a, b);
        assert_ne!(a, other);
        assert_eq!(a.get_version(), Some(uuid::Version::Sha1));
        let own = Uuid::new_v4();
        assert_eq!(user_uuid_for_subject(&own.to_string()), own);
    }
}
