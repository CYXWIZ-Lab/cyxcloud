//! New node registrations are limited per client address, and a client
//! cannot dodge the limit by naming its own X-Forwarded-For. Needs a running
//! gateway with Postgres and Redis and no proxy in front (TRUSTED_PROXY_HOPS
//! unset), whose limit counter for this address is fresh (it registers
//! nodes in that gateway's database):
//!   CYXCLOUD_TEST_GATEWAY_GRPC=http://127.0.0.1:50160 \
//!     cargo test -p cyxcloud-gateway --test registration_limit -- --ignored

use cyxcloud_core::node_identity::NodeKey;
use cyxcloud_protocol::node::{
    node_service_client::NodeServiceClient, NodeInfo, NodeProof, RegisterNodeRequest,
};

fn signed(key: &NodeKey) -> tonic::Request<RegisterNodeRequest> {
    let addr = "127.0.0.1:59999";
    let at = chrono::Utc::now().timestamp();
    tonic::Request::new(RegisterNodeRequest {
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
        access_secret: String::new(),
    })
}

#[tokio::test]
#[ignore = "needs a running gateway: set CYXCLOUD_TEST_GATEWAY_GRPC"]
async fn new_nodes_are_limited_per_address_and_known_nodes_are_not() {
    let url = std::env::var("CYXCLOUD_TEST_GATEWAY_GRPC")
        .expect("CYXCLOUD_TEST_GATEWAY_GRPC=http://host:port");
    let mut client = NodeServiceClient::connect(url).await.expect("gateway");

    let first = NodeKey::generate();
    let others: Vec<NodeKey> = (1..5).map(|_| NodeKey::generate()).collect();
    for (i, key) in std::iter::once(&first).chain(&others).enumerate() {
        let mut req = signed(key);
        // a different made-up client address each time: ignored
        req.metadata_mut()
            .insert("x-forwarded-for", format!("203.0.113.{i}").parse().unwrap());
        let r = client.register_node(req).await.expect("within the limit");
        assert!(r.get_ref().success, "{}", r.get_ref().error);
    }

    let mut sixth = signed(&NodeKey::generate());
    sixth
        .metadata_mut()
        .insert("x-forwarded-for", "203.0.113.99".parse().unwrap());
    let refused = client
        .register_node(sixth)
        .await
        .expect_err("over the limit");
    assert_eq!(
        refused.code(),
        tonic::Code::ResourceExhausted,
        "{refused:?}"
    );

    // a node on file restarting is not counted
    let again = client
        .register_node(signed(&first))
        .await
        .expect("known node");
    assert!(again.get_ref().success, "{}", again.get_ref().error);
}
