//! Authentication REST API endpoints
//!
//! Provides endpoints for:
//! - Wallet-based authentication
//! - API key management
//! - Token refresh

#![allow(unused_imports)]

use crate::auth::{
    AuthResponse, AuthService, AuthUser, ChallengeResponse, Claims, CreateApiKeyRequest, TokenType,
    WalletLoginRequest,
};
use crate::AppState;
use axum::{
    extract::{Json, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// API error response
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: code.into(),
        }
    }
}

/// Create authentication routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Challenge for wallet auth
        .route("/challenge", get(get_challenge))
        // Wallet login
        .route("/wallet", post(wallet_login))
        // Refresh token
        .route("/refresh", post(refresh_token))
        // API keys
        .route("/api-keys", post(create_api_key))
        // Logout (revoke token)
        .route("/logout", post(logout))
        // Get current user info
        .route("/me", get(get_me))
}

/// Get a challenge message for wallet authentication
async fn get_challenge(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ChallengeResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let (nonce, message) = auth.generate_challenge();

    // Challenge valid for 5 minutes
    let expires_at = chrono::Utc::now().timestamp() + 300;

    Ok(Json(ChallengeResponse {
        nonce,
        message,
        expires_at,
    }))
}

/// Login with wallet signature
async fn wallet_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WalletLoginRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();

    info!(wallet = %req.wallet_address, "Wallet login attempt");

    // Verify the signature
    match auth.verify_wallet_signature(&req.wallet_address, req.message.as_bytes(), &req.signature)
    {
        Ok(true) => {
            debug!(wallet = %req.wallet_address, "Signature verified");
        }
        Ok(false) | Err(_) => {
            warn!(wallet = %req.wallet_address, "Invalid signature");
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ApiError::new("Invalid signature", "INVALID_SIGNATURE")),
            ));
        }
    }

    // Get or create user in database
    let user_id = if let Some(meta) = state.metadata_service() {
        match meta.get_or_create_user(&req.wallet_address).await {
            Ok(user) => user.id.to_string(),
            Err(e) => {
                error!(error = %e, "Failed to get/create user");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError::new("Database error", "DB_ERROR")),
                ));
            }
        }
    } else {
        // In-memory mode: use wallet address as user ID
        req.wallet_address.clone()
    };

    // Generate tokens
    let default_permissions = vec![
        "storage:read".to_string(),
        "storage:write".to_string(),
        "dataset:read".to_string(),
    ];

    let access_token = auth
        .generate_token(
            &user_id,
            TokenType::Access,
            Some(req.wallet_address.clone()),
            default_permissions.clone(),
        )
        .map_err(|e| {
            error!(error = %e, "Failed to generate access token");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Token generation failed", "TOKEN_ERROR")),
            )
        })?;

    let refresh_token = auth
        .generate_token(
            &user_id,
            TokenType::Refresh,
            Some(req.wallet_address.clone()),
            default_permissions,
        )
        .map_err(|e| {
            error!(error = %e, "Failed to generate refresh token");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Token generation failed", "TOKEN_ERROR")),
            )
        })?;

    info!(user_id = %user_id, wallet = %req.wallet_address, "Wallet login successful");

    Ok(Json(AuthResponse {
        access_token,
        refresh_token: Some(refresh_token),
        token_type: "Bearer".to_string(),
        expires_in: TokenType::Access.lifetime_secs(),
        user_id,
    }))
}

/// Request body for token refresh
#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

/// Refresh access token using refresh token
async fn refresh_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshTokenRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();

    // Validate refresh token
    let claims = auth.validate_token(&req.refresh_token).await.map_err(|e| {
        warn!(error = %e, "Invalid refresh token");
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new("Invalid refresh token", "INVALID_TOKEN")),
        )
    })?;

    // Generate new access token
    let access_token = auth
        .generate_token(
            &claims.sub,
            TokenType::Access,
            claims.wallet.clone(),
            claims.permissions.clone(),
        )
        .map_err(|e| {
            error!(error = %e, "Failed to generate access token");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Token generation failed", "TOKEN_ERROR")),
            )
        })?;

    debug!(user_id = %claims.sub, "Token refreshed");

    Ok(Json(AuthResponse {
        access_token,
        refresh_token: None, // Don't issue new refresh token
        token_type: "Bearer".to_string(),
        expires_in: TokenType::Access.lifetime_secs(),
        user_id: claims.sub,
    }))
}

/// Create an API key
async fn create_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<Json<ApiKeyResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();

    // Authenticate the request
    let claims = extract_and_validate_token(&headers, auth).await?;

    // Generate API key token
    let expires_days = req.expires_in_days.unwrap_or(365);
    let permissions = if req.permissions.is_empty() {
        claims.permissions.clone()
    } else {
        req.permissions
    };

    let api_key = auth
        .generate_token(&claims.sub, TokenType::ApiKey, claims.wallet, permissions)
        .map_err(|e| {
            error!(error = %e, "Failed to generate API key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("API key generation failed", "TOKEN_ERROR")),
            )
        })?;

    info!(user_id = %claims.sub, name = %req.name, "API key created");

    Ok(Json(ApiKeyResponse {
        name: req.name,
        api_key,
        expires_in_days: expires_days,
    }))
}

/// Response for API key creation
#[derive(Debug, Serialize)]
pub struct ApiKeyResponse {
    pub name: String,
    pub api_key: String,
    pub expires_in_days: i64,
}

/// Logout (revoke current token)
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();

    // Get and validate token
    let claims = extract_and_validate_token(&headers, auth).await?;

    // Revoke the token
    auth.revoke_token(&claims.jti).await;

    info!(user_id = %claims.sub, "User logged out");

    Ok(StatusCode::NO_CONTENT)
}

/// Get current user info
async fn get_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<UserInfoResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();

    // Get and validate token
    let claims = extract_and_validate_token(&headers, auth).await?;

    Ok(Json(UserInfoResponse {
        user_id: claims.sub,
        user_type: claims.user_type,
        wallet: claims.wallet,
        permissions: claims.permissions,
    }))
}

/// Response for user info
#[derive(Debug, Serialize)]
pub struct UserInfoResponse {
    pub user_id: String,
    pub user_type: String,
    pub wallet: Option<String>,
    pub permissions: Vec<String>,
}

/// Extract and validate JWT from Authorization header
async fn extract_and_validate_token(
    headers: &HeaderMap,
    auth: &AuthService,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // Get Authorization header
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ApiError::new(
                    "Missing Authorization header",
                    "MISSING_AUTH",
                )),
            )
        })?;

    // Check Bearer prefix
    if !auth_header.starts_with("Bearer ") {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(
                "Invalid Authorization format",
                "INVALID_AUTH_FORMAT",
            )),
        ));
    }

    let token = &auth_header[7..];

    // Validate token
    auth.validate_token(token).await.map_err(|e| {
        warn!(error = %e, "Token validation failed");
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(format!("{}", e), "INVALID_TOKEN")),
        )
    })
}

/// Axum extractor for authenticated user
pub struct AuthenticatedUser(pub AuthUser);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error() {
        let error = ApiError::new("Test error", "TEST_CODE");
        assert_eq!(error.error, "Test error");
        assert_eq!(error.code, "TEST_CODE");
    }
}
