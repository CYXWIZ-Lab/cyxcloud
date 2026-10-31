//! The admin site's view of storage (its Drive page): totals and the storage
//! nodes, and holding a node in maintenance.
//!
//! Every request carries an admin token: HS256, signed with
//! `ADMIN_JWT_SECRET` (the secret the admin site signs central's admin
//! tokens with), issued by `cyxwiz-admin-dashboard`, `role: "admin"`, not
//! expired. Without `ADMIN_JWT_SECRET` the routes answer 503: the admin API
//! is off.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::AppState;

/// An admin secret shorter than this turns the admin API off (HS256 keys
/// need 256 bits; the same rule as central's).
const MIN_ADMIN_SECRET_BYTES: usize = 32;

/// The issuer the admin site signs its service tokens with.
const ADMIN_TOKEN_ISSUER: &str = "cyxwiz-admin-dashboard";

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// Whether `authorization` carries an admin token signed with `key`.
fn is_admin(key: &DecodingKey, authorization: Option<&str>) -> bool {
    let Some(token) = authorization
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    validation.set_required_spec_claims(&["exp", "iss"]);
    validation.set_issuer(&[ADMIN_TOKEN_ISSUER]);
    match decode::<serde_json::Value>(token, key, &validation) {
        Ok(data) => data.claims.get("role").and_then(|r| r.as_str()) == Some("admin"),
        Err(e) => {
            debug!(error = %e, "admin API request with a bad token");
            false
        }
    }
}

async fn require_admin(key: Option<Arc<DecodingKey>>, req: Request, next: Next) -> Response {
    let Some(key) = key else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the admin API is off");
    };
    let authorization = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !is_admin(&key, authorization) {
        return refuse(StatusCode::UNAUTHORIZED, "an admin token is required");
    }
    next.run(req).await
}

/// The admin routes, nested at `/api/v1/admin`. `admin_secret`: the
/// `ADMIN_JWT_SECRET` value, `None` when unset.
pub fn routes(admin_secret: Option<String>) -> Router<Arc<AppState>> {
    let key = admin_secret
        .filter(|s| s.len() >= MIN_ADMIN_SECRET_BYTES)
        .map(|s| Arc::new(DecodingKey::from_secret(s.as_bytes())));
    if key.is_none() {
        warn!(
            "ADMIN_JWT_SECRET unset or shorter than {MIN_ADMIN_SECRET_BYTES} bytes:              the admin API (/api/v1/admin) answers 503"
        );
    }
    Router::new()
        .route("/stats", get(stats))
        .route("/nodes", get(nodes))
        .route("/nodes/:id/maintenance", post(maintenance))
        .route_layer(middleware::from_fn(move |req: Request, next: Next| {
            require_admin(key.clone(), req, next)
        }))
}

/// One storage node as the admin site shows it.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AdminNode {
    pub id: Uuid,
    pub peer_id: String,
    pub grpc_address: String,
    pub status: String,
    pub storage_total: i64,
    pub storage_reserved: i64,
    pub storage_used: i64,
    pub storage_available: i64,
    pub chunk_count: i64,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub region: Option<String>,
    pub version: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct NodeCounts {
    pub total: usize,
    pub online: usize,
    pub offline: usize,
    /// Any other status: draining, maintenance, recovering
    pub other: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct StorageTotals {
    /// Bytes the online nodes offer, less what each keeps reserved
    pub total: i64,
    /// Bytes stored on every node, online or not
    pub used: i64,
    /// Bytes the online nodes can still take
    pub available: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AdminStats {
    pub nodes: NodeCounts,
    pub storage: StorageTotals,
    /// Chunk copies stored across the nodes
    pub chunks: i64,
}

impl AdminStats {
    fn of(nodes: &[AdminNode]) -> Self {
        let mut s = AdminStats::default();
        for n in nodes {
            s.nodes.total += 1;
            match n.status.as_str() {
                "online" => s.nodes.online += 1,
                "offline" => s.nodes.offline += 1,
                _ => s.nodes.other += 1,
            }
            s.storage.used += n.storage_used;
            if n.status == "online" {
                s.storage.total += (n.storage_total - n.storage_reserved).max(0);
                s.storage.available += n.storage_available;
            }
            s.chunks += n.chunk_count;
        }
        s
    }
}

/// Every node with its stored chunks, newest first.
async fn all_nodes(state: &AppState) -> Result<Vec<AdminNode>, Response> {
    let Some(meta) = state.metadata_service() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the gateway has no metadata store",
        ));
    };
    let db = meta.database();
    let failed = |e: cyxcloud_metadata::DbError| {
        warn!(error = %e, "admin API could not read the nodes");
        refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read the nodes",
        )
    };
    let summary = db.get_node_storage_summary().await.map_err(failed)?;
    let details: HashMap<Uuid, _> = db
        .get_all_nodes()
        .await
        .map_err(failed)?
        .into_iter()
        .map(|n| (n.id, n))
        .collect();
    let mut nodes: Vec<AdminNode> = summary
        .into_iter()
        .map(|s| {
            let d = details.get(&s.id);
            AdminNode {
                id: s.id,
                peer_id: s.peer_id,
                grpc_address: s.grpc_address,
                status: s.status,
                storage_total: s.storage_total,
                storage_reserved: s.storage_reserved,
                storage_used: s.storage_used,
                storage_available: s.storage_available,
                chunk_count: s.chunk_count,
                last_heartbeat: s.last_heartbeat,
                region: d.and_then(|n| n.region.clone()),
                version: d.and_then(|n| n.version.clone()),
                created_at: d.map(|n| n.created_at),
            }
        })
        .collect();
    nodes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(nodes)
}

/// GET /api/v1/admin/stats
async fn stats(State(state): State<Arc<AppState>>) -> Response {
    match all_nodes(&state).await {
        Ok(nodes) => Json(AdminStats::of(&nodes)).into_response(),
        Err(r) => r,
    }
}

#[derive(Debug, Deserialize)]
pub struct NodesQuery {
    /// Only nodes with this status (`all` or empty: every node)
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
struct NodesResponse {
    nodes: Vec<AdminNode>,
    total: usize,
}

/// GET /api/v1/admin/nodes[?status=online]
async fn nodes(State(state): State<Arc<AppState>>, Query(q): Query<NodesQuery>) -> Response {
    let mut nodes = match all_nodes(&state).await {
        Ok(nodes) => nodes,
        Err(r) => return r,
    };
    if let Some(status) = q.status.filter(|s| !s.is_empty() && s != "all") {
        nodes.retain(|n| n.status == status);
    }
    let total = nodes.len();
    Json(NodesResponse { nodes, total }).into_response()
}

#[derive(Debug, Deserialize)]
pub struct MaintenanceRequest {
    /// true: hold the node in maintenance; false: release it
    pub on: bool,
}

/// POST /api/v1/admin/nodes/:id/maintenance {"on": bool} - hold a node in
/// maintenance (no new chunks, still serves what it has, heartbeats and
/// restarts keep it there) or release it. 409 when the node's status does not
/// allow the change (only online or recovering nodes can be held).
async fn maintenance(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<MaintenanceRequest>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return refuse(StatusCode::BAD_REQUEST, "the node id is not a UUID");
    };
    let Some(meta) = state.metadata_service() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the gateway has no metadata store",
        );
    };
    let exists = match meta.database().get_node(id).await {
        Ok(node) => node.is_some(),
        Err(e) => {
            warn!(error = %e, "admin API could not read the node");
            return refuse(StatusCode::INTERNAL_SERVER_ERROR, "could not read the node");
        }
    };
    if !exists {
        return refuse(StatusCode::NOT_FOUND, "no such node");
    }
    match meta.set_node_maintenance(id, req.on).await {
        Ok(Some(node)) => {
            info!(node_id = %id, status = %node.status, "admin changed a node's maintenance");
            Json(serde_json::json!({ "id": node.id, "status": node.status })).into_response()
        }
        Ok(None) => refuse(
            StatusCode::CONFLICT,
            if req.on {
                "only an online or recovering node can be put in maintenance"
            } else {
                "the node is not in maintenance"
            },
        ),
        Err(e) => {
            warn!(error = %e, "admin API could not change the node");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not change the node",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use tower::ServiceExt;

    const SECRET: &str = "admin-secret-admin-secret-admin-secret";

    fn signed(header: Header, secret: &str, claims: serde_json::Value) -> String {
        encode(
            &header,
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    fn claims(role: &str, exp_offset: i64) -> serde_json::Value {
        serde_json::json!({
            "sub": "admin-dashboard",
            "iss": ADMIN_TOKEN_ISSUER,
            "role": role,
            "exp": Utc::now().timestamp() + exp_offset,
        })
    }

    fn token(secret: &str, role: &str, exp_offset: i64) -> String {
        signed(Header::default(), secret, claims(role, exp_offset))
    }

    async fn status(secret: Option<&str>, authorization: Option<String>) -> StatusCode {
        let state = Arc::new(AppState::new());
        let app = routes(secret.map(String::from)).with_state(state);
        let mut req = HttpRequest::builder().uri("/stats");
        if let Some(a) = authorization {
            req = req.header(header::AUTHORIZATION, a);
        }
        app.oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn only_an_admin_token_gets_in() {
        assert_eq!(status(Some(SECRET), None).await, StatusCode::UNAUTHORIZED);
        let user = format!("Bearer {}", token(SECRET, "user", 300));
        assert_eq!(
            status(Some(SECRET), Some(user)).await,
            StatusCode::UNAUTHORIZED
        );
        let other = format!("Bearer {}", token("someone-else", "admin", 300));
        assert_eq!(
            status(Some(SECRET), Some(other)).await,
            StatusCode::UNAUTHORIZED
        );
        let expired = format!("Bearer {}", token(SECRET, "admin", -3600));
        assert_eq!(
            status(Some(SECRET), Some(expired)).await,
            StatusCode::UNAUTHORIZED
        );
        let mut no_exp = claims("admin", 300);
        no_exp.as_object_mut().unwrap().remove("exp");
        let no_exp = format!("Bearer {}", signed(Header::default(), SECRET, no_exp));
        assert_eq!(
            status(Some(SECRET), Some(no_exp)).await,
            StatusCode::UNAUTHORIZED
        );
        let mut other_issuer = claims("admin", 300);
        other_issuer["iss"] = "cyxwiz-central".into();
        let other_issuer = format!("Bearer {}", signed(Header::default(), SECRET, other_issuer));
        assert_eq!(
            status(Some(SECRET), Some(other_issuer)).await,
            StatusCode::UNAUTHORIZED
        );
        let hs384 = signed(Header::new(Algorithm::HS384), SECRET, claims("admin", 300));
        let hs384 = format!("Bearer {hs384}");
        assert_eq!(
            status(Some(SECRET), Some(hs384)).await,
            StatusCode::UNAUTHORIZED
        );
        let basic = format!("Basic {}", token(SECRET, "admin", 300));
        assert_eq!(
            status(Some(SECRET), Some(basic)).await,
            StatusCode::UNAUTHORIZED
        );
        // In memory there are no nodes to read: past the check, 503.
        let admin = format!("Bearer {}", token(SECRET, "admin", 300));
        assert_eq!(
            status(Some(SECRET), Some(admin)).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn maintenance_needs_an_admin_and_a_node_id() {
        let state = Arc::new(AppState::new());
        let app = routes(Some(SECRET.to_string())).with_state(state);
        let post = |auth: Option<String>, id: &str| {
            let mut req = HttpRequest::builder()
                .method("POST")
                .uri(format!("/nodes/{id}/maintenance"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(a) = auth {
                req = req.header(header::AUTHORIZATION, a);
            }
            req.body(Body::from(r#"{"on":true}"#)).unwrap()
        };
        let id = Uuid::new_v4().to_string();
        let user = format!("Bearer {}", token(SECRET, "user", 300));
        let admin = format!("Bearer {}", token(SECRET, "admin", 300));
        let status = |r: Response| r.status();
        assert_eq!(
            status(app.clone().oneshot(post(None, &id)).await.unwrap()),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(app.clone().oneshot(post(Some(user), &id)).await.unwrap()),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(
                app.clone()
                    .oneshot(post(Some(admin.clone()), "not-a-uuid"))
                    .await
                    .unwrap()
            ),
            StatusCode::BAD_REQUEST
        );
        // In memory there is no metadata store: past the checks, 503.
        assert_eq!(
            status(app.oneshot(post(Some(admin), &id)).await.unwrap()),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn without_the_secret_the_api_is_off() {
        let admin = format!("Bearer {}", token(SECRET, "admin", 300));
        assert_eq!(
            status(None, Some(admin)).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let short = "short-secret";
        let admin = format!("Bearer {}", token(short, "admin", 300));
        assert_eq!(
            status(Some(short), Some(admin)).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    fn node(status: &str, total: i64, reserved: i64, used: i64, chunks: i64) -> AdminNode {
        AdminNode {
            id: Uuid::new_v4(),
            peer_id: "p".into(),
            grpc_address: "a".into(),
            status: status.into(),
            storage_total: total,
            storage_reserved: reserved,
            storage_used: used,
            storage_available: (total - reserved - used).max(0),
            chunk_count: chunks,
            last_heartbeat: None,
            region: None,
            version: None,
            created_at: None,
        }
    }

    #[test]
    fn stats_add_up_the_nodes() {
        let s = AdminStats::of(&[
            node("online", 100, 10, 30, 4),
            node("offline", 50, 0, 5, 1),
            node("draining", 20, 0, 0, 0),
        ]);
        assert_eq!(
            s.nodes,
            NodeCounts {
                total: 3,
                online: 1,
                offline: 1,
                other: 1
            }
        );
        assert_eq!(
            s.storage,
            StorageTotals {
                total: 90,
                used: 35,
                available: 60
            }
        );
        assert_eq!(s.chunks, 5);
    }
}
