//! Audit Logging for Security Events
//!
//! Structured audit log for security-relevant events.
//! Outputs as JSON to tracing (can be routed to file/SIEM via tracing subscriber).

use chrono::Utc;
use serde::Serialize;
use tracing::info;

/// Audit event types
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEvent {
    /// User authentication attempt
    AuthAttempt {
        method: String, // "wallet", "api_key", "refresh"
        success: bool,
        user_id: Option<String>,
        ip: Option<String>,
        reason: Option<String>,
    },

    /// Token revoked
    TokenRevoked {
        jti: String,
        user_id: String,
        ip: Option<String>,
    },

    /// API key created
    ApiKeyCreated {
        key_id: String,
        user_id: String,
        permissions: Vec<String>,
    },

    /// API key deleted
    ApiKeyDeleted {
        key_id: String,
        user_id: String,
    },

    /// Rate limit exceeded
    RateLimited {
        endpoint: String,
        ip: String,
        limit: u64,
    },

    /// Path traversal attempt blocked
    PathTraversalBlocked {
        key: String,
        ip: Option<String>,
    },

    /// TLS/mTLS event
    TlsEvent {
        event_type: String, // "client_cert_verified", "client_cert_rejected"
        subject: Option<String>,
    },

    /// Node registration
    NodeRegistered {
        node_id: String,
        ip: Option<String>,
    },

    /// Node deregistered or expired
    NodeRemoved {
        node_id: String,
        reason: String,
    },

    /// Administrative action
    AdminAction {
        action: String,
        user_id: String,
        details: Option<String>,
    },
}

/// Structured audit log entry
#[derive(Debug, Serialize)]
struct AuditLogEntry {
    timestamp: String,
    event_type: String,
    event: AuditEvent,
}

/// Emit a structured audit log entry via tracing
pub fn audit_log(event: AuditEvent) {
    let event_type = match &event {
        AuditEvent::AuthAttempt { .. } => "auth_attempt",
        AuditEvent::TokenRevoked { .. } => "token_revoked",
        AuditEvent::ApiKeyCreated { .. } => "api_key_created",
        AuditEvent::ApiKeyDeleted { .. } => "api_key_deleted",
        AuditEvent::RateLimited { .. } => "rate_limited",
        AuditEvent::PathTraversalBlocked { .. } => "path_traversal_blocked",
        AuditEvent::TlsEvent { .. } => "tls_event",
        AuditEvent::NodeRegistered { .. } => "node_registered",
        AuditEvent::NodeRemoved { .. } => "node_removed",
        AuditEvent::AdminAction { .. } => "admin_action",
    };

    let entry = AuditLogEntry {
        timestamp: Utc::now().to_rfc3339(),
        event_type: event_type.to_string(),
        event,
    };

    // Emit as structured JSON via tracing at INFO level with "audit" target
    if let Ok(json) = serde_json::to_string(&entry) {
        info!(target: "audit", "{}", json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent::AuthAttempt {
            method: "wallet".to_string(),
            success: true,
            user_id: Some("user-123".to_string()),
            ip: Some("127.0.0.1".to_string()),
            reason: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("auth_attempt"));
        assert!(json.contains("wallet"));
        assert!(json.contains("user-123"));
    }

    #[test]
    fn test_rate_limit_event() {
        let event = AuditEvent::RateLimited {
            endpoint: "/api/v1/auth/challenge".to_string(),
            ip: "10.0.0.1".to_string(),
            limit: 10,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("rate_limited"));
        assert!(json.contains("10.0.0.1"));
    }

    #[test]
    fn test_path_traversal_event() {
        let event = AuditEvent::PathTraversalBlocked {
            key: "../etc/passwd".to_string(),
            ip: Some("192.168.1.1".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("path_traversal_blocked"));
        assert!(json.contains("../etc/passwd"));
    }
}
