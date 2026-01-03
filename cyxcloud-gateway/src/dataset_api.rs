//! Dataset REST API endpoints
//!
//! Provides endpoints for:
//! - Listing user's datasets
//! - Listing public datasets (MNIST, CIFAR, etc.)
//! - Creating datasets from uploaded files
//! - Verifying dataset integrity
//! - Sharing datasets with other users

use crate::auth::{AuthService, Claims};
use crate::public_registry::{PublicDatasetRegistry, PublicDatasetSummary};
use crate::verification::VerificationService;
use crate::AppState;
use axum::{
    extract::{Json, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use cyxcloud_metadata::{CreateDataset, TrustLevel};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

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

/// Dataset info response
#[derive(Debug, Serialize)]
pub struct DatasetResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: String,
    pub file_count: i32,
    pub size_bytes: i64,
    pub content_hash: Vec<u8>,
    pub trust_level: i32,
    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// Public dataset info response
#[derive(Debug, Serialize)]
pub struct PublicDatasetResponse {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub verified_by: Vec<String>,
    pub cached: bool,
}

impl From<PublicDatasetSummary> for PublicDatasetResponse {
    fn from(pd: PublicDatasetSummary) -> Self {
        Self {
            id: pd.id.to_string(),
            name: pd.name,
            version: pd.version,
            license: pd.license,
            verified_by: pd.verified_by,
            cached: pd.cached,
        }
    }
}

/// Dataset verification result response
#[derive(Debug, Serialize)]
pub struct VerificationResponse {
    pub manifest_valid: bool,
    pub all_files_valid: bool,
    pub files_verified: i32,
    pub files_failed: i32,
    pub trust_level: i32,
    pub public_match: Option<PublicMatchResponse>,
}

/// Public dataset match response
#[derive(Debug, Serialize)]
pub struct PublicMatchResponse {
    pub name: String,
    pub version: String,
    pub verified_by: Vec<String>,
    pub license: Option<String>,
}

/// Dataset share result response
#[derive(Debug, Serialize)]
pub struct ShareResponse {
    pub success: bool,
    pub share_id: String,
}

/// Query params for list datasets
#[derive(Debug, Deserialize)]
pub struct ListDatasetsQuery {
    pub include_shared: Option<bool>,
    pub limit: Option<i32>,
}

/// Query params for list public datasets
#[derive(Debug, Deserialize)]
pub struct ListPublicQuery {
    pub filter: Option<String>,
}

/// Query params for verify dataset
#[derive(Debug, Deserialize)]
pub struct VerifyQuery {
    pub check_public: Option<bool>,
    pub full: Option<bool>,
}

/// Request body for create dataset
#[derive(Debug, Deserialize)]
pub struct CreateDatasetRequest {
    pub name: String,
    pub description: Option<String>,
    pub bucket: String,
    pub prefix: Option<String>,
}

/// Request body for share dataset
#[derive(Debug, Deserialize)]
pub struct ShareDatasetRequest {
    pub share_with: String,
    pub permissions: Vec<String>,
}

/// Create dataset routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // List user's datasets
        .route("/", get(list_datasets).post(create_dataset))
        // List public datasets
        .route("/public", get(list_public_datasets))
        // Get dataset info
        .route("/{dataset_id}", get(get_dataset_info))
        // Verify dataset
        .route("/{dataset_id}/verify", post(verify_dataset))
        // Share dataset
        .route("/{dataset_id}/share", post(share_dataset))
}

/// List user's datasets
async fn list_datasets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListDatasetsQuery>,
) -> Result<Json<Vec<DatasetResponse>>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let claims = extract_and_validate_token(&headers, auth).await?;

    let metadata = state.metadata_service().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new("Metadata service not available", "SERVICE_UNAVAILABLE")),
        )
    })?;

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid user ID", "INVALID_USER_ID")),
        )
    })?;

    let include_shared = query.include_shared.unwrap_or(false);
    let limit = query.limit.unwrap_or(100);

    let datasets = metadata
        .database()
        .list_user_datasets(user_id, include_shared, limit)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list datasets");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to list datasets", "DB_ERROR")),
            )
        })?;

    let response: Vec<DatasetResponse> = datasets
        .into_iter()
        .map(|d| DatasetResponse {
            id: d.id.to_string(),
            name: d.name,
            description: d.description,
            owner_id: d.owner_id.to_string(),
            file_count: d.file_count,
            size_bytes: d.total_size_bytes,
            content_hash: d.content_hash,
            trust_level: d.trust_level,
            version: d.version,
            created_at: d.created_at.to_rfc3339(),
            updated_at: d.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(response))
}

/// List public datasets
async fn list_public_datasets(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListPublicQuery>,
) -> Result<Json<Vec<PublicDatasetResponse>>, (StatusCode, Json<ApiError>)> {
    let registry = PublicDatasetRegistry::new(state);

    let datasets = registry
        .list_public_datasets(query.filter.as_deref())
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list public datasets");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to list public datasets", "REGISTRY_ERROR")),
            )
        })?;

    let response: Vec<PublicDatasetResponse> = datasets
        .into_iter()
        .map(PublicDatasetResponse::from)
        .collect();

    Ok(Json(response))
}

/// Create a dataset from files in a bucket
async fn create_dataset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateDatasetRequest>,
) -> Result<Json<DatasetResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let claims = extract_and_validate_token(&headers, auth).await?;

    let metadata = state.metadata_service().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new("Metadata service not available", "SERVICE_UNAVAILABLE")),
        )
    })?;

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid user ID", "INVALID_USER_ID")),
        )
    })?;

    // List files in the bucket with optional prefix to calculate dataset stats
    let files = metadata
        .database()
        .list_files_by_bucket_prefix(&req.bucket, req.prefix.as_deref(), 10000)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list files");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to list files", "DB_ERROR")),
            )
        })?;

    if files.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "No files found in bucket with the specified prefix",
                "NO_FILES",
            )),
        ));
    }

    // Calculate dataset stats
    let file_count = files.len() as i32;
    let size_bytes: i64 = files.iter().map(|f| f.size_bytes).sum();

    // Compute content hash from all file hashes (sorted for determinism)
    let mut sorted_hashes: Vec<&[u8]> = files.iter().map(|f| f.content_hash.as_slice()).collect();
    sorted_hashes.sort();
    let mut hasher = blake3::Hasher::new();
    for h in sorted_hashes {
        hasher.update(h);
    }
    let content_hash = hasher.finalize().as_bytes().to_vec();

    // Create the dataset
    let create_dataset = CreateDataset {
        name: req.name.clone(),
        description: req.description.clone(),
        owner_id: user_id,
        content_hash: content_hash.clone(),
        total_size_bytes: size_bytes,
        file_count,
        schema: None,
        trust_level: TrustLevel::SelfUploaded,
        signature: None,
        parent_version_id: None,
    };

    let dataset = metadata
        .database()
        .create_dataset(create_dataset)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to create dataset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to create dataset", "DB_ERROR")),
            )
        })?;

    info!(
        dataset_id = %dataset.id,
        name = %req.name,
        file_count = file_count,
        "Dataset created"
    );

    Ok(Json(DatasetResponse {
        id: dataset.id.to_string(),
        name: dataset.name,
        description: dataset.description,
        owner_id: dataset.owner_id.to_string(),
        file_count: dataset.file_count,
        size_bytes: dataset.total_size_bytes,
        content_hash: dataset.content_hash,
        trust_level: dataset.trust_level,
        version: dataset.version,
        created_at: dataset.created_at.to_rfc3339(),
        updated_at: dataset.updated_at.to_rfc3339(),
    }))
}

/// Get dataset info
async fn get_dataset_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(dataset_id): Path<String>,
) -> Result<Json<DatasetResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let claims = extract_and_validate_token(&headers, auth).await?;

    let metadata = state.metadata_service().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new("Metadata service not available", "SERVICE_UNAVAILABLE")),
        )
    })?;

    let dataset_uuid = Uuid::parse_str(&dataset_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid dataset ID", "INVALID_DATASET_ID")),
        )
    })?;

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid user ID", "INVALID_USER_ID")),
        )
    })?;

    let dataset = metadata
        .database()
        .get_dataset(dataset_uuid)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to get dataset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to get dataset", "DB_ERROR")),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("Dataset not found", "NOT_FOUND")),
            )
        })?;

    // Check access (owner or shared)
    if dataset.owner_id != user_id {
        let has_access = metadata
            .database()
            .check_dataset_access(dataset_uuid, user_id)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to check dataset access");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError::new("Failed to check access", "DB_ERROR")),
                )
            })?;

        if has_access.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ApiError::new("Access denied", "FORBIDDEN")),
            ));
        }
    }

    Ok(Json(DatasetResponse {
        id: dataset.id.to_string(),
        name: dataset.name,
        description: dataset.description,
        owner_id: dataset.owner_id.to_string(),
        file_count: dataset.file_count,
        size_bytes: dataset.total_size_bytes,
        content_hash: dataset.content_hash,
        trust_level: dataset.trust_level,
        version: dataset.version,
        created_at: dataset.created_at.to_rfc3339(),
        updated_at: dataset.updated_at.to_rfc3339(),
    }))
}

/// Verify a dataset's integrity
async fn verify_dataset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(dataset_id): Path<String>,
    Query(query): Query<VerifyQuery>,
) -> Result<Json<VerificationResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let claims = extract_and_validate_token(&headers, auth).await?;

    let dataset_uuid = Uuid::parse_str(&dataset_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid dataset ID", "INVALID_DATASET_ID")),
        )
    })?;

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid user ID", "INVALID_USER_ID")),
        )
    })?;

    // Check dataset access first
    let metadata = state.metadata_service().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new("Metadata service not available", "SERVICE_UNAVAILABLE")),
        )
    })?;

    let dataset = metadata
        .database()
        .get_dataset(dataset_uuid)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to get dataset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to get dataset", "DB_ERROR")),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("Dataset not found", "NOT_FOUND")),
            )
        })?;

    if dataset.owner_id != user_id {
        let has_access = metadata
            .database()
            .check_dataset_access(dataset_uuid, user_id)
            .await
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError::new("Failed to check access", "DB_ERROR")),
                )
            })?;

        if has_access.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ApiError::new("Access denied", "FORBIDDEN")),
            ));
        }
    }

    let full_verification = query.full.unwrap_or(false);
    let check_public = query.check_public.unwrap_or(true);

    // Perform verification
    let verification_service = VerificationService::new(state.clone());
    let result = verification_service
        .verify_dataset(dataset_uuid, full_verification)
        .await
        .map_err(|e| {
            error!(error = %e, "Verification failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Verification failed", "VERIFICATION_ERROR")),
            )
        })?;

    // Check against public registry if requested
    let public_match = if check_public {
        let registry = PublicDatasetRegistry::new(state);
        registry
            .verify_dataset(dataset_uuid)
            .await
            .ok()
            .flatten()
            .map(|m| PublicMatchResponse {
                name: m.name,
                version: m.version,
                verified_by: m.verified_by,
                license: m.license,
            })
    } else {
        None
    };

    Ok(Json(VerificationResponse {
        manifest_valid: result.manifest_valid,
        all_files_valid: result.all_files_valid,
        files_verified: result.files_verified as i32,
        files_failed: result.files_failed as i32,
        trust_level: result.computed_trust_level as i32,
        public_match,
    }))
}

/// Share a dataset with another user
async fn share_dataset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(dataset_id): Path<String>,
    Json(req): Json<ShareDatasetRequest>,
) -> Result<Json<ShareResponse>, (StatusCode, Json<ApiError>)> {
    let auth = state.auth_service();
    let claims = extract_and_validate_token(&headers, auth).await?;

    let metadata = state.metadata_service().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new("Metadata service not available", "SERVICE_UNAVAILABLE")),
        )
    })?;

    let dataset_uuid = Uuid::parse_str(&dataset_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid dataset ID", "INVALID_DATASET_ID")),
        )
    })?;

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Invalid user ID", "INVALID_USER_ID")),
        )
    })?;

    // Verify user owns the dataset
    let dataset = metadata
        .database()
        .get_dataset(dataset_uuid)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to get dataset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to get dataset", "DB_ERROR")),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("Dataset not found", "NOT_FOUND")),
            )
        })?;

    if dataset.owner_id != user_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("Only the owner can share a dataset", "FORBIDDEN")),
        ));
    }

    // Find target user
    let target_user = metadata
        .database()
        .get_user_by_email_or_wallet(&req.share_with)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to find user");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to find user", "DB_ERROR")),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("User not found", "USER_NOT_FOUND")),
            )
        })?;

    // Create share
    let share = metadata
        .database()
        .share_dataset(dataset_uuid, target_user.id, req.permissions)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to share dataset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("Failed to share dataset", "DB_ERROR")),
            )
        })?;

    info!(
        dataset_id = %dataset_uuid,
        shared_with = %target_user.id,
        "Dataset shared"
    );

    Ok(Json(ShareResponse {
        success: true,
        share_id: share.id.to_string(),
    }))
}

/// Extract and validate JWT from Authorization header
async fn extract_and_validate_token(
    headers: &HeaderMap,
    auth: &AuthService,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ApiError::new("Missing Authorization header", "MISSING_AUTH")),
            )
        })?;

    if !auth_header.starts_with("Bearer ") {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new("Invalid Authorization format", "INVALID_AUTH_FORMAT")),
        ));
    }

    let token = &auth_header[7..];

    auth.validate_token(token).await.map_err(|e| {
        warn!(error = %e, "Token validation failed");
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(format!("{}", e), "INVALID_TOKEN")),
        )
    })
}

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
