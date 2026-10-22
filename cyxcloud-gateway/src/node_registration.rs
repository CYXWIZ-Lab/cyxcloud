//! What the gateway checks before it believes a node's registration
//! (TOFIX176 P2): the request is signed by the key the node id was derived
//! from, and a node id already on file keeps the key it was registered with.
//!
//! Pure: no database, no clock of its own, so it is tested without either.

use cyxcloud_core::node_identity::{NodeIdentityError, RegistrationProof};
use cyxcloud_protocol::node::RegisterNodeRequest;
use tonic::Status;

/// A registration the gateway may act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRegistration {
    pub node_id: String,
    pub public_key_hex: String,
    pub listen_addr: String,
    /// The secret the gateway must present on chunk RPCs to this node, if the
    /// node sent one.
    pub access_secret: Option<String>,
}

/// Check a registration against the key already on file for that node id
/// (`known_public_key`, None for a node never seen) at time `now` (Unix s).
pub fn check_registration(
    req: &RegisterNodeRequest,
    known_public_key: Option<&str>,
    now: i64,
) -> Result<VerifiedRegistration, Status> {
    let info = req
        .info
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("NodeInfo is required"))?;

    let listen_addr = info
        .listen_addrs
        .first()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Status::invalid_argument("the node must say where it listens (listen_addrs)"))?;

    let proof = req.proof.as_ref().ok_or_else(|| {
        Status::invalid_argument(
            "registration is not signed: this node has no node key. Upgrade cyxcloud-node and run `cyxcloud node init`",
        )
    })?;

    if info.public_key.is_empty() {
        return Err(Status::invalid_argument(
            "registration carries no public key (NodeInfo.public_key)",
        ));
    }

    RegistrationProof {
        node_id: &req.node_id,
        public_key_hex: &info.public_key,
        listen_addr,
        timestamp: proof.timestamp,
        signature: &proof.signature,
    }
    .verify(now)
    .map_err(|e| match e {
        NodeIdentityError::StaleTimestamp { .. } => Status::failed_precondition(format!(
            "{e}: check the node's clock"
        )),
        other => Status::permission_denied(other.to_string()),
    })?;

    if let Some(known) = known_public_key {
        if !known.is_empty() && !known.eq_ignore_ascii_case(&info.public_key) {
            return Err(Status::permission_denied(format!(
                "node id {} is registered with another key; a node keeps the key it registered with",
                req.node_id
            )));
        }
    }

    Ok(VerifiedRegistration {
        node_id: req.node_id.clone(),
        public_key_hex: info.public_key.to_ascii_lowercase(),
        listen_addr: listen_addr.clone(),
        access_secret: if req.access_secret.is_empty() {
            None
        } else {
            Some(req.access_secret.clone())
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyxcloud_core::node_identity::NodeKey;
    use cyxcloud_protocol::node::{NodeInfo, NodeProof};

    const ADDR: &str = "192.168.1.180:50051";
    const NOW: i64 = 1_800_000_000;

    fn signed_request(key: &NodeKey, addr: &str, at: i64) -> RegisterNodeRequest {
        RegisterNodeRequest {
            node_id: key.node_id(),
            info: Some(NodeInfo {
                node_id: key.node_id(),
                public_key: key.public_key_hex(),
                listen_addrs: vec![addr.to_string()],
                ..Default::default()
            }),
            proof: Some(NodeProof {
                timestamp: at,
                signature: key.sign_registration(addr, at),
            }),
            access_secret: "s3cret".to_string(),
        }
    }

    #[test]
    fn a_signed_registration_from_a_new_node_is_accepted() {
        let key = NodeKey::generate();
        let req = signed_request(&key, ADDR, NOW);

        let v = check_registration(&req, None, NOW + 5).unwrap();

        assert_eq!(v.node_id, key.node_id());
        assert_eq!(v.public_key_hex, key.public_key_hex());
        assert_eq!(v.listen_addr, ADDR);
        assert_eq!(v.access_secret.as_deref(), Some("s3cret"));
    }

    #[test]
    fn the_same_node_re_registering_with_its_key_is_accepted() {
        let key = NodeKey::generate();
        let req = signed_request(&key, "10.0.0.5:50051", NOW);

        let v = check_registration(&req, Some(&key.public_key_hex()), NOW).unwrap();

        assert_eq!(v.listen_addr, "10.0.0.5:50051");
    }

    #[test]
    fn an_unsigned_registration_is_refused_with_the_remedy() {
        let key = NodeKey::generate();
        let mut req = signed_request(&key, ADDR, NOW);
        req.proof = None;

        let err = check_registration(&req, None, NOW).unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("cyxcloud node init"), "{}", err.message());
    }

    #[test]
    fn a_known_node_id_with_another_key_is_refused() {
        let owner = NodeKey::generate();
        let thief = NodeKey::generate();
        // the thief signs correctly with its own key but claims the owner's id
        let mut req = signed_request(&thief, ADDR, NOW);
        req.node_id = owner.node_id();
        req.info.as_mut().unwrap().node_id = owner.node_id();

        let err = check_registration(&req, Some(&owner.public_key_hex()), NOW).unwrap_err();

        // refused before the key comparison: the id is not derived from the thief's key
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn a_node_cannot_change_its_key() {
        let old = NodeKey::generate();
        let new = NodeKey::generate();
        let req = signed_request(&new, ADDR, NOW);

        // the row for new.node_id() somehow holds another key: refuse
        let err = check_registration(&req, Some(&old.public_key_hex()), NOW).unwrap_err();

        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        assert!(err.message().contains("another key"));
    }

    #[test]
    fn a_stale_registration_is_a_clock_problem() {
        let key = NodeKey::generate();
        let req = signed_request(&key, ADDR, NOW - 3600);

        let err = check_registration(&req, None, NOW).unwrap_err();

        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(err.message().contains("clock"));
    }

    #[test]
    fn a_registration_without_a_listen_address_is_refused() {
        let key = NodeKey::generate();
        let mut req = signed_request(&key, ADDR, NOW);
        req.info.as_mut().unwrap().listen_addrs.clear();

        let err = check_registration(&req, None, NOW).unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn a_legacy_row_without_a_key_takes_the_first_signed_key() {
        let key = NodeKey::generate();
        let req = signed_request(&key, ADDR, NOW);

        let v = check_registration(&req, Some(""), NOW).unwrap();

        assert_eq!(v.public_key_hex, key.public_key_hex());
    }
}
