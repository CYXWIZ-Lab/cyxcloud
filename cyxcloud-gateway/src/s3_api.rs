//! S3-Compatible REST API
//!
//! Implements a subset of the AWS S3 API for object storage operations.
//! Supports: PUT, GET, DELETE, HEAD, and LIST operations.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, head, put},
    Router,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument};

use crate::AppState;

/// S3 API error types
#[derive(Error, Debug)]
pub enum S3Error {
    #[error("Bucket not found: {0}")]
    NoSuchBucket(String),

    #[error("Key not found: {0}")]
    NoSuchKey(String),

    #[error("Bucket already exists: {0}")]
    BucketAlreadyExists(String),

    #[error("Bucket not empty: {0}")]
    BucketNotEmpty(String),

    #[error("Access denied")]
    AccessDenied,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match &self {
            S3Error::NoSuchBucket(b) => (
                StatusCode::NOT_FOUND,
                "NoSuchBucket",
                format!("Bucket {} does not exist", b),
            ),
            S3Error::NoSuchKey(k) => (
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                format!("Key {} does not exist", k),
            ),
            S3Error::BucketAlreadyExists(b) => (
                StatusCode::CONFLICT,
                "BucketAlreadyExists",
                format!("Bucket {} already exists", b),
            ),
            S3Error::BucketNotEmpty(b) => (
                StatusCode::CONFLICT,
                "BucketNotEmpty",
                format!("Bucket {} is not empty", b),
            ),
            S3Error::AccessDenied => (
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "Access Denied".to_string(),
            ),
            S3Error::InvalidRequest(m) => (StatusCode::BAD_REQUEST, "InvalidRequest", m.clone()),
            S3Error::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                m.clone(),
            ),
        };

        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Error>
    <Code>{}</Code>
    <Message>{}</Message>
</Error>"#,
            error_code, message
        );

        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/xml")
            .body(Body::from(body))
            .unwrap()
    }
}

pub type S3Result<T> = Result<T, S3Error>;

/// Query parameters for list objects
#[derive(Debug, Deserialize)]
pub struct ListObjectsQuery {
    #[serde(rename = "list-type")]
    pub list_type: Option<i32>,
    pub prefix: Option<String>,
    pub delimiter: Option<String>,
    #[serde(rename = "max-keys")]
    pub max_keys: Option<i32>,
    #[serde(rename = "continuation-token")]
    pub continuation_token: Option<String>,
    #[serde(rename = "start-after")]
    pub start_after: Option<String>,
}

/// Object metadata for listings
#[derive(Debug, Serialize)]
pub struct ObjectInfo {
    pub key: String,
    pub last_modified: String,
    pub etag: String,
    pub size: u64,
    pub storage_class: String,
}

/// List objects response
#[derive(Debug, Serialize)]
pub struct ListObjectsV2Response {
    pub name: String,
    pub prefix: Option<String>,
    pub max_keys: i32,
    pub is_truncated: bool,
    pub contents: Vec<ObjectInfo>,
    pub common_prefixes: Vec<String>,
    pub continuation_token: Option<String>,
    pub next_continuation_token: Option<String>,
    pub key_count: i32,
}

impl ListObjectsV2Response {
    fn to_xml(&self) -> String {
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        xml.push_str("\n<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        xml.push_str(&format!("\n  <Name>{}</Name>", self.name));

        if let Some(prefix) = &self.prefix {
            xml.push_str(&format!("\n  <Prefix>{}</Prefix>", prefix));
        } else {
            xml.push_str("\n  <Prefix/>");
        }

        xml.push_str(&format!("\n  <MaxKeys>{}</MaxKeys>", self.max_keys));
        xml.push_str(&format!(
            "\n  <IsTruncated>{}</IsTruncated>",
            self.is_truncated
        ));
        xml.push_str(&format!("\n  <KeyCount>{}</KeyCount>", self.key_count));

        for obj in &self.contents {
            xml.push_str("\n  <Contents>");
            xml.push_str(&format!("\n    <Key>{}</Key>", obj.key));
            xml.push_str(&format!(
                "\n    <LastModified>{}</LastModified>",
                obj.last_modified
            ));
            xml.push_str(&format!("\n    <ETag>{}</ETag>", obj.etag));
            xml.push_str(&format!("\n    <Size>{}</Size>", obj.size));
            xml.push_str(&format!(
                "\n    <StorageClass>{}</StorageClass>",
                obj.storage_class
            ));
            xml.push_str("\n  </Contents>");
        }

        for prefix in &self.common_prefixes {
            xml.push_str("\n  <CommonPrefixes>");
            xml.push_str(&format!("\n    <Prefix>{}</Prefix>", prefix));
            xml.push_str("\n  </CommonPrefixes>");
        }

        if let Some(token) = &self.next_continuation_token {
            xml.push_str(&format!(
                "\n  <NextContinuationToken>{}</NextContinuationToken>",
                token
            ));
        }

        xml.push_str("\n</ListBucketResult>");
        xml
    }
}

/// Create S3 API routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Bucket operations
        .route("/:bucket", put(create_bucket))
        .route("/:bucket", delete(delete_bucket))
        .route("/:bucket", head(head_bucket))
        .route("/:bucket", get(list_objects))
        // Object operations
        .route("/:bucket/*key", put(put_object))
        .route("/:bucket/*key", get(get_object))
        .route("/:bucket/*key", delete(delete_object))
        .route("/:bucket/*key", head(head_object))
}

// =============================================================================
// BUCKET OPERATIONS
// =============================================================================

/// PUT /:bucket - Create bucket
#[instrument(skip(state))]
async fn create_bucket(
    State(state): State<Arc<AppState>>,
    Path(bucket): Path<String>,
) -> S3Result<impl IntoResponse> {
    info!(bucket = %bucket, "Creating bucket");

    // Check if bucket exists
    if state.bucket_exists(&bucket).await? {
        return Err(S3Error::BucketAlreadyExists(bucket));
    }

    // Create bucket in metadata
    state.create_bucket(&bucket).await?;

    Ok((StatusCode::OK, [(header::LOCATION, format!("/{}", bucket))]))
}

/// DELETE /:bucket - Delete bucket
#[instrument(skip(state))]
async fn delete_bucket(
    State(state): State<Arc<AppState>>,
    Path(bucket): Path<String>,
) -> S3Result<impl IntoResponse> {
    info!(bucket = %bucket, "Deleting bucket");

    // Check if bucket exists
    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    // Check if bucket is empty
    if !state.bucket_is_empty(&bucket).await? {
        return Err(S3Error::InvalidRequest("Bucket is not empty".to_string()));
    }

    // Delete bucket
    state.delete_bucket(&bucket).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// HEAD /:bucket - Check if bucket exists
#[instrument(skip(state))]
async fn head_bucket(
    State(state): State<Arc<AppState>>,
    Path(bucket): Path<String>,
) -> S3Result<impl IntoResponse> {
    debug!(bucket = %bucket, "Checking bucket");

    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    Ok(StatusCode::OK)
}

/// GET /:bucket - List objects in bucket
#[instrument(skip(state))]
async fn list_objects(
    State(state): State<Arc<AppState>>,
    Path(bucket): Path<String>,
    Query(query): Query<ListObjectsQuery>,
) -> S3Result<impl IntoResponse> {
    debug!(bucket = %bucket, prefix = ?query.prefix, "Listing objects");

    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    let max_keys = query.max_keys.unwrap_or(1000).min(1000);
    let prefix = query.prefix.clone().unwrap_or_default();
    let delimiter = query.delimiter.clone();

    // Get objects from metadata
    let (objects, is_truncated, next_token) = state
        .list_objects(
            &bucket,
            &prefix,
            delimiter.as_deref(),
            max_keys,
            query.continuation_token.as_deref(),
        )
        .await?;

    // Build response
    let response = ListObjectsV2Response {
        name: bucket,
        prefix: Some(prefix),
        max_keys,
        is_truncated,
        key_count: objects.len() as i32,
        contents: objects,
        common_prefixes: Vec::new(), // TODO: Implement delimiter handling
        continuation_token: query.continuation_token,
        next_continuation_token: next_token,
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        response.to_xml(),
    ))
}

// =============================================================================
// OBJECT OPERATIONS
// =============================================================================

/// PUT /:bucket/*key - Upload object
#[instrument(skip(state, body))]
async fn put_object(
    State(state): State<Arc<AppState>>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> S3Result<impl IntoResponse> {
    info!(bucket = %bucket, key = %key, size = body.len(), "Uploading object");

    // Validate bucket exists
    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    // Get content type from headers
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    // Store object
    let etag = state.put_object(&bucket, &key, body, &content_type).await?;

    Ok((StatusCode::OK, [(header::ETAG, format!("\"{}\"", etag))]))
}

/// GET /:bucket/*key - Download object
#[instrument(skip(state))]
async fn get_object(
    State(state): State<Arc<AppState>>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> S3Result<Response> {
    debug!(bucket = %bucket, key = %key, "Getting object");

    // Validate bucket exists
    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    // Get object metadata
    let metadata = state
        .get_object_metadata(&bucket, &key)
        .await?
        .ok_or_else(|| S3Error::NoSuchKey(key.clone()))?;

    // Check for Range header
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range_header(s, metadata.size));

    // Get object data
    let (data, status) = if let Some((start, end)) = range {
        let partial = state.get_object_range(&bucket, &key, start, end).await?;
        (partial, StatusCode::PARTIAL_CONTENT)
    } else {
        let full = state.get_object(&bucket, &key).await?;
        (full, StatusCode::OK)
    };

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &metadata.content_type)
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::ETAG, format!("\"{}\"", metadata.etag))
        .header(header::LAST_MODIFIED, &metadata.last_modified);

    if let Some((start, end)) = range {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, metadata.size),
        );
    }

    response
        .body(Body::from(data))
        .map_err(|e| S3Error::Internal(e.to_string()))
}

/// DELETE /:bucket/*key - Delete object
#[instrument(skip(state))]
async fn delete_object(
    State(state): State<Arc<AppState>>,
    Path((bucket, key)): Path<(String, String)>,
) -> S3Result<impl IntoResponse> {
    info!(bucket = %bucket, key = %key, "Deleting object");

    // Validate bucket exists
    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    // Delete object (idempotent - don't error if not found)
    state.delete_object(&bucket, &key).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// HEAD /:bucket/*key - Get object metadata
#[instrument(skip(state))]
async fn head_object(
    State(state): State<Arc<AppState>>,
    Path((bucket, key)): Path<(String, String)>,
) -> S3Result<Response> {
    debug!(bucket = %bucket, key = %key, "Head object");

    // Validate bucket exists
    if !state.bucket_exists(&bucket).await? {
        return Err(S3Error::NoSuchBucket(bucket));
    }

    // Get object metadata
    let metadata = state
        .get_object_metadata(&bucket, &key)
        .await?
        .ok_or_else(|| S3Error::NoSuchKey(key.clone()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &metadata.content_type)
        .header(header::CONTENT_LENGTH, metadata.size)
        .header(header::ETAG, format!("\"{}\"", metadata.etag))
        .header(header::LAST_MODIFIED, &metadata.last_modified)
        .body(Body::empty())
        .map_err(|e| S3Error::Internal(e.to_string()))
}

// =============================================================================
// HELPERS
// =============================================================================

/// Parse Range header (e.g., "bytes=0-999")
fn parse_range_header(header: &str, total_size: u64) -> Option<(u64, u64)> {
    let header = header.strip_prefix("bytes=")?;
    let parts: Vec<&str> = header.split('-').collect();

    if parts.len() != 2 {
        return None;
    }

    let (start, end): (u64, u64) = if parts[0].is_empty() {
        // Suffix range: -500 means last 500 bytes
        let suffix: u64 = parts[1].parse().ok()?;
        (total_size.saturating_sub(suffix), total_size - 1)
    } else {
        let start: u64 = parts[0].parse().ok()?;
        let end: u64 = if parts[1].is_empty() {
            total_size - 1
        } else {
            parts[1].parse().ok()?
        };
        (start, end)
    };

    if start > end || start >= total_size {
        return None;
    }

    Some((start, end.min(total_size - 1)))
}

/// Object metadata returned by storage
#[derive(Debug, Clone)]
pub struct ObjectMetadata {
    pub key: String,
    pub size: u64,
    pub content_type: String,
    pub etag: String,
    pub last_modified: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_header() {
        // Full range
        assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));

        // Open-ended range
        assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));

        // Suffix range
        assert_eq!(parse_range_header("bytes=-500", 1000), Some((500, 999)));

        // Invalid range
        assert_eq!(parse_range_header("bytes=500-100", 1000), None);

        // Out of bounds
        assert_eq!(parse_range_header("bytes=1500-", 1000), None);
    }

    #[test]
    fn test_list_objects_response_xml() {
        let response = ListObjectsV2Response {
            name: "test-bucket".to_string(),
            prefix: Some("prefix/".to_string()),
            max_keys: 1000,
            is_truncated: false,
            key_count: 1,
            contents: vec![ObjectInfo {
                key: "prefix/file.txt".to_string(),
                last_modified: "2024-01-01T00:00:00Z".to_string(),
                etag: "abc123".to_string(),
                size: 1024,
                storage_class: "STANDARD".to_string(),
            }],
            common_prefixes: Vec::new(),
            continuation_token: None,
            next_continuation_token: None,
        };

        let xml = response.to_xml();
        assert!(xml.contains("<Name>test-bucket</Name>"));
        assert!(xml.contains("<Key>prefix/file.txt</Key>"));
        assert!(xml.contains("<Size>1024</Size>"));
    }
}
