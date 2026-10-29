//! Which service a request goes to.

use crate::config::Route;

/// The route for a path: the longest prefix of the right kind (gRPC requests
/// only reach gRPC routes, HTTP requests only HTTP routes), or none.
pub fn route_for<'a>(routes: &'a [Route], path: &str, grpc: bool) -> Option<&'a Route> {
    routes
        .iter()
        .filter(|r| r.grpc == grpc && path.starts_with(&r.prefix))
        .max_by_key(|r| r.prefix.len())
}

/// Whether a request is gRPC (by its content type, as gRPC defines it).
pub fn is_grpc(headers: &hyper::HeaderMap) -> bool {
    headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/grpc"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_routes;

    fn name(path: &str, grpc: bool) -> Option<String> {
        route_for(&default_routes(), path, grpc).map(|r| format!("{} {}", r.name, r.upstream))
    }

    #[test]
    fn grpc_goes_by_package() {
        assert_eq!(
            name("/cyxcloud.datastream.DataStreamService/ListDatasets", true).unwrap(),
            "gateway http://127.0.0.1:50060"
        );
        assert_eq!(
            name("/cyxwiz.protocol.JobService/SubmitJob", true).unwrap(),
            "central http://127.0.0.1:50051"
        );
        assert_eq!(name("/grpc.health.v1.Health/Check", true), None);
    }

    #[test]
    fn http_goes_by_the_longest_prefix() {
        assert_eq!(
            name("/api/datasets/abc/verify", false).unwrap(),
            "gateway http://127.0.0.1:8180"
        );
        assert_eq!(
            name("/api/v1/auth/challenge", false).unwrap(),
            "gateway http://127.0.0.1:8180"
        );
        assert_eq!(
            name("/api/auth/login", false).unwrap(),
            "api http://127.0.0.1:3002"
        );
        assert_eq!(
            name("/api/machines/register", false).unwrap(),
            "api http://127.0.0.1:3002"
        );
    }

    #[test]
    fn what_must_stay_inside_is_not_routed() {
        for path in ["/s3/bucket/key", "/metrics", "/health/x", "/ws", "/version"] {
            assert_eq!(name(path, false), None, "{}", path);
        }
        // a gRPC path is not reachable as plain HTTP, nor the reverse
        assert_eq!(name("/cyxcloud.node.NodeService/RegisterNode", false), None);
        assert_eq!(name("/api/users/me", true), None);
    }
}
