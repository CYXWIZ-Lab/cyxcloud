//! Integration tests for CyxCloud Gateway
//!
//! Tests S3 API, authentication, and failure scenarios using in-memory storage.
//! Run with: cargo test --test integration_tests -p cyxcloud-gateway

use bytes::Bytes;
use std::sync::Arc;

use cyxcloud_gateway::auth::TokenType;
use cyxcloud_gateway::{AppState, AuthService};

// ============================================================================
// Auth Service Tests
// ============================================================================

#[tokio::test]
async fn test_auth_token_lifecycle() {
    let auth = AuthService::from_env();

    // Generate access token
    let token = auth
        .generate_token("test-user-id", TokenType::Access, None, vec!["read".into(), "write".into()])
        .expect("Failed to generate token");

    assert!(!token.is_empty());

    // Validate
    let claims = auth.validate_token(&token).await.expect("Failed to validate");
    assert_eq!(claims.sub, "test-user-id");
    assert_eq!(claims.user_type, "user");
    assert!(claims.permissions.contains(&"read".to_string()));
    assert!(claims.permissions.contains(&"write".to_string()));
}

#[tokio::test]
async fn test_auth_token_revocation() {
    let auth = AuthService::from_env();

    let token = auth
        .generate_token("test-user", TokenType::Access, None, vec!["read".into()])
        .expect("Failed to generate token");

    let claims = auth.validate_token(&token).await.expect("Should be valid");

    // Revoke
    auth.revoke_token(&claims.jti).await;

    // Should now fail
    let result = auth.validate_token(&token).await;
    assert!(result.is_err(), "Revoked token should be rejected");
}

#[tokio::test]
async fn test_auth_invalid_token() {
    let auth = AuthService::from_env();

    assert!(auth.validate_token("not-a-valid-jwt").await.is_err());
    assert!(auth.validate_token("").await.is_err());
}

#[tokio::test]
async fn test_auth_permissions() {
    let auth = AuthService::from_env();

    let token = auth
        .generate_token("admin", TokenType::Access, None, vec!["read".into(), "write".into(), "admin".into()])
        .expect("Failed to generate token");

    let claims = auth.validate_token(&token).await.expect("Valid token");

    assert!(AuthService::has_permission(&claims, "admin"));
    assert!(AuthService::has_permission(&claims, "read"));
    assert!(!AuthService::has_permission(&claims, "superuser"));
    assert!(AuthService::has_any_permission(&claims, &["superuser", "admin"]));
    assert!(!AuthService::has_any_permission(&claims, &["superuser", "root"]));
}

#[tokio::test]
async fn test_auth_challenge_generation() {
    let auth = AuthService::from_env();

    let (challenge, nonce) = auth.generate_challenge();
    assert!(!challenge.is_empty());
    assert!(!nonce.is_empty());

    let (_, nonce2) = auth.generate_challenge();
    assert_ne!(nonce, nonce2);
}

#[tokio::test]
async fn test_auth_multiple_revocations() {
    let auth = AuthService::from_env();

    let mut tokens = Vec::new();
    for i in 0..10 {
        let token = auth
            .generate_token(&format!("user-{}", i), TokenType::Access, None, vec!["read".into()])
            .expect("Failed to generate token");
        let claims = auth.validate_token(&token).await.expect("Valid");
        tokens.push((token, claims.jti));
    }

    // Revoke odd-numbered tokens
    for (i, (_, jti)) in tokens.iter().enumerate() {
        if i % 2 == 1 {
            auth.revoke_token(jti).await;
        }
    }

    // Verify
    for (i, (token, _)) in tokens.iter().enumerate() {
        let result = auth.validate_token(token).await;
        if i % 2 == 0 {
            assert!(result.is_ok(), "Token {} should be valid", i);
        } else {
            assert!(result.is_err(), "Token {} should be revoked", i);
        }
    }
}

#[tokio::test]
async fn test_auth_different_token_types() {
    let auth = AuthService::from_env();

    let access = auth
        .generate_token("user", TokenType::Access, None, vec![])
        .expect("access token");
    let node = auth
        .generate_token("node-1", TokenType::Node, None, vec!["compute".into()])
        .expect("node token");
    let api_key = auth
        .generate_token("user", TokenType::ApiKey, None, vec!["read".into()])
        .expect("api key token");

    // All should validate
    assert!(auth.validate_token(&access).await.is_ok());
    assert!(auth.validate_token(&node).await.is_ok());
    assert!(auth.validate_token(&api_key).await.is_ok());
}

// ============================================================================
// AppState In-Memory Storage Tests
// ============================================================================

#[tokio::test]
async fn test_bucket_create_and_exists() {
    let state = Arc::new(AppState::new());

    assert!(!state.bucket_exists("test-bucket").await.unwrap());
    state.create_bucket("test-bucket").await.unwrap();
    assert!(state.bucket_exists("test-bucket").await.unwrap());
}

#[tokio::test]
async fn test_bucket_duplicate_create() {
    let state = Arc::new(AppState::new());

    state.create_bucket("dup-bucket").await.unwrap();
    let result = state.create_bucket("dup-bucket").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_object_put_get_delete() {
    let state = Arc::new(AppState::new());
    state.create_bucket("mybucket").await.unwrap();

    let data = Bytes::from("hello world");
    let etag = state
        .put_object("mybucket", "test.txt", data.clone(), "text/plain")
        .await
        .unwrap();
    assert!(!etag.is_empty());

    let retrieved = state.get_object("mybucket", "test.txt").await.unwrap();
    assert_eq!(retrieved, data);

    let meta = state
        .get_object_metadata("mybucket", "test.txt")
        .await
        .unwrap();
    assert!(meta.is_some());
    let meta = meta.unwrap();
    assert_eq!(meta.size, 11);
    assert_eq!(meta.content_type, "text/plain");

    state.delete_object("mybucket", "test.txt").await.unwrap();

    let meta = state
        .get_object_metadata("mybucket", "test.txt")
        .await
        .unwrap();
    assert!(meta.is_none());
}

#[tokio::test]
async fn test_object_overwrite() {
    let state = Arc::new(AppState::new());
    state.create_bucket("bucket").await.unwrap();

    state
        .put_object("bucket", "key", Bytes::from("v1"), "text/plain")
        .await
        .unwrap();
    state
        .put_object("bucket", "key", Bytes::from("version2"), "text/plain")
        .await
        .unwrap();

    let data = state.get_object("bucket", "key").await.unwrap();
    assert_eq!(data, Bytes::from("version2"));
}

#[tokio::test]
async fn test_object_in_nonexistent_bucket() {
    let state = Arc::new(AppState::new());

    let result = state
        .put_object("no-such-bucket", "key", Bytes::from("data"), "text/plain")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_nonexistent_object() {
    let state = Arc::new(AppState::new());
    state.create_bucket("bucket").await.unwrap();

    let result = state.get_object("bucket", "missing-key").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_objects_empty_bucket() {
    let state = Arc::new(AppState::new());
    state.create_bucket("empty").await.unwrap();

    let (objects, is_truncated, next_token) = state
        .list_objects("empty", "", None, 1000, None)
        .await
        .unwrap();

    assert!(objects.is_empty());
    assert!(!is_truncated);
    assert!(next_token.is_none());
}

#[tokio::test]
async fn test_list_objects_with_prefix() {
    let state = Arc::new(AppState::new());
    state.create_bucket("bucket").await.unwrap();

    state
        .put_object("bucket", "docs/readme.md", Bytes::from("readme"), "text/plain")
        .await
        .unwrap();
    state
        .put_object("bucket", "docs/guide.md", Bytes::from("guide"), "text/plain")
        .await
        .unwrap();
    state
        .put_object("bucket", "images/logo.png", Bytes::from("img"), "image/png")
        .await
        .unwrap();

    let (objects, _, _) = state
        .list_objects("bucket", "docs/", None, 1000, None)
        .await
        .unwrap();

    assert_eq!(objects.len(), 2);
    for obj in &objects {
        assert!(obj.key.starts_with("docs/"));
    }
}

#[tokio::test]
async fn test_delete_bucket_non_empty() {
    let state = Arc::new(AppState::new());
    state.create_bucket("bucket").await.unwrap();
    state
        .put_object("bucket", "file.txt", Bytes::from("data"), "text/plain")
        .await
        .unwrap();

    assert!(!state.bucket_is_empty("bucket").await.unwrap());
}

#[tokio::test]
async fn test_delete_bucket_empty() {
    let state = Arc::new(AppState::new());
    state.create_bucket("bucket").await.unwrap();

    assert!(state.bucket_is_empty("bucket").await.unwrap());
    state.delete_bucket("bucket").await.unwrap();
    assert!(!state.bucket_exists("bucket").await.unwrap());
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

#[tokio::test]
async fn test_concurrent_uploads() {
    let state = Arc::new(AppState::new());
    state.create_bucket("concurrent").await.unwrap();

    let mut handles = Vec::new();
    for i in 0..50 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("file-{}.txt", i);
            let data = Bytes::from(format!("content-{}", i));
            s.put_object("concurrent", &key, data, "text/plain")
                .await
                .unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    let (objects, _, _) = state
        .list_objects("concurrent", "", None, 1000, None)
        .await
        .unwrap();
    assert_eq!(objects.len(), 50);
}

#[tokio::test]
async fn test_concurrent_read_write() {
    let state = Arc::new(AppState::new());
    state.create_bucket("rw").await.unwrap();
    state
        .put_object("rw", "shared.txt", Bytes::from("initial"), "text/plain")
        .await
        .unwrap();

    let mut handles = Vec::new();

    for _ in 0..20 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let data = s.get_object("rw", "shared.txt").await.unwrap();
            assert!(!data.is_empty());
        }));
    }

    for i in 0..10 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let data = Bytes::from(format!("update-{}", i));
            s.put_object("rw", "shared.txt", data, "text/plain")
                .await
                .unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    let data = state.get_object("rw", "shared.txt").await.unwrap();
    assert!(!data.is_empty());
}

// ============================================================================
// Large Object Test
// ============================================================================

#[tokio::test]
async fn test_large_object() {
    let state = Arc::new(AppState::new());
    state.create_bucket("large").await.unwrap();

    let data = Bytes::from(vec![42u8; 1024 * 1024]);
    state
        .put_object("large", "big.bin", data.clone(), "application/octet-stream")
        .await
        .unwrap();

    let retrieved = state.get_object("large", "big.bin").await.unwrap();
    assert_eq!(retrieved.len(), 1024 * 1024);
    assert_eq!(retrieved, data);
}

// ============================================================================
// Range Retrieval Test
// ============================================================================

#[tokio::test]
async fn test_range_retrieval() {
    let state = Arc::new(AppState::new());
    state.create_bucket("range").await.unwrap();

    let data = Bytes::from("0123456789ABCDEF");
    state
        .put_object("range", "data.txt", data, "text/plain")
        .await
        .unwrap();

    let partial = state
        .get_object_range("range", "data.txt", 0, 4)
        .await
        .unwrap();
    assert_eq!(partial, Bytes::from("01234"));
}

// ============================================================================
// Auth + State Combined
// ============================================================================

#[tokio::test]
async fn test_auth_from_state() {
    let state = Arc::new(AppState::new());
    let auth = state.auth_service_arc();

    let token = auth
        .generate_token("state-user", TokenType::Access, None, vec!["read".into()])
        .expect("Should generate token");

    let claims = auth.validate_token(&token).await.expect("Should validate");
    assert_eq!(claims.sub, "state-user");
}
