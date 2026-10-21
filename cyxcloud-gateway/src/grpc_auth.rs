//! gRPC authentication as a tower layer (TOFIX164 slice 1b).
//!
//! The layer sees the real request path, so which methods may run without a
//! JWT is decided by the server, never by a header the client sets. Every
//! other method needs `authorization: Bearer <JWT>`; its validated claims go
//! into the request extensions for the handlers (RequestClaimsExt).

use crate::auth::{AuthService, Claims};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::BoxBody;
use tonic::codegen::http::{Request, Response};
use tonic::Status;
use tower::{Layer, Service};

/// Methods a caller may use without a JWT:
/// - RegisterNode is a storage node's first call;
/// - DataService/StreamData carries its own credential, the dataset access
///   token in the request body, which the handler validates (no token, no
///   bytes).
pub const METHODS_WITHOUT_JWT: &[&str] = &[
    "/cyxcloud.node.NodeService/RegisterNode",
    "/cyxcloud.data.DataService/StreamData",
];

/// Whether a request path needs a validated JWT.
pub fn needs_jwt(path: &str) -> bool {
    !METHODS_WITHOUT_JWT.contains(&path)
}

/// The token in an `authorization` header value, or why there is none.
pub fn bearer_token(header: Option<&str>) -> Result<&str, Status> {
    let value = header.ok_or_else(|| Status::unauthenticated("Missing authorization header"))?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            Status::unauthenticated("Invalid authorization format (expected 'Bearer <token>')")
        })
}

/// Adds [`AuthMiddleware`] around the gRPC routes.
#[derive(Clone)]
pub struct AuthLayer {
    auth: Arc<AuthService>,
}

impl AuthLayer {
    pub fn new(auth: Arc<AuthService>) -> Self {
        Self { auth }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            auth: self.auth.clone(),
        }
    }
}

/// Validates the JWT of every request whose path needs one.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    auth: Arc<AuthService>,
}

impl<S, B> Service<Request<B>> for AuthMiddleware<S>
where
    S: Service<Request<B>, Response = Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        // The ready inner service goes into the future; a clone stays here.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let auth = self.auth.clone();
        Box::pin(async move {
            if needs_jwt(request.uri().path()) {
                let header = request
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let token = match bearer_token(header.as_deref()) {
                    Ok(token) => token.to_owned(),
                    Err(status) => return Ok(status.to_http()),
                };
                let claims: Claims = match auth.validate_token(&token).await {
                    Ok(claims) => claims,
                    Err(e) => {
                        return Ok(Status::unauthenticated(format!("Invalid token: {}", e)).to_http())
                    }
                };
                request.extensions_mut().insert(claims);
            }
            inner.call(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_named_methods_run_without_a_jwt() {
        assert!(!needs_jwt("/cyxcloud.node.NodeService/RegisterNode"));
        assert!(!needs_jwt("/cyxcloud.data.DataService/StreamData"));
        assert!(needs_jwt("/cyxcloud.node.NodeService/Heartbeat"));
        assert!(needs_jwt("/cyxcloud.data.DataService/GetDataset"));
        assert!(needs_jwt("/cyxcloud.datastream.DataStreamService/CreateAccessToken"));
        assert!(needs_jwt("/cyxcloud.datastream.DataStreamService/StreamBatches"));
        assert!(needs_jwt(""));
    }

    #[test]
    fn bearer_tokens_are_read_strictly() {
        assert_eq!(bearer_token(Some("Bearer abc.def")).unwrap(), "abc.def");
        assert_eq!(bearer_token(None).unwrap_err().code(), tonic::Code::Unauthenticated);
        assert_eq!(bearer_token(Some("abc")).unwrap_err().code(), tonic::Code::Unauthenticated);
        assert_eq!(bearer_token(Some("Bearer ")).unwrap_err().code(), tonic::Code::Unauthenticated);
        assert!(bearer_token(Some("basic abc")).is_err());
    }
}
