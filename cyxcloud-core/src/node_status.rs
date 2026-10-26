//! The status report a storage node serves at `GET /status` on its local
//! metrics port, and `cyxcloud node status` renders (TOFIX177). One definition
//! for both sides of the wire. No secrets in it.

use serde::{Deserialize, Serialize};

/// The machine the node runs on, as the OS reports it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MachineFacts {
    pub hostname: String,
    /// e.g. "Arch Linux 6.17" or "Windows 11 (26200)"
    pub os: String,
    /// e.g. "Intel(R) Core(TM) i7-2620M CPU @ 2.70GHz"
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub ram_bytes: u64,
}

/// Storage the node offers and holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StorageFacts {
    pub data_dir: String,
    pub offered_bytes: u64,
    pub used_bytes: u64,
    pub chunks: u64,
    /// Free space on the disk that holds `data_dir` (0 when unknown)
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
}

/// The node's standing with its server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerFacts {
    /// The gateway address the node registers with
    pub address: String,
    pub registered: bool,
    /// Unix seconds of the last heartbeat the server acknowledged (0 = none yet)
    pub last_heartbeat_unix: i64,
    /// What went wrong last, in words (empty when all is well)
    pub last_error: String,
}

/// Everything `cyxcloud node status` renders.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NodeStatusReport {
    pub version: String,
    pub node_id: String,
    pub name: String,
    pub listen_addr: String,
    pub started_unix: i64,
    pub machine: MachineFacts,
    pub storage: StorageFacts,
    pub server: ServerFacts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_round_trips_as_json_and_carries_no_secrets() {
        let report = NodeStatusReport {
            version: "0.1.0".into(),
            node_id: "4856c6439493904b62111f6cc3af3c46".into(),
            name: "hp-linux".into(),
            listen_addr: "192.168.1.180:50051".into(),
            started_unix: 1_800_000_000,
            machine: MachineFacts {
                hostname: "cyxwiz".into(),
                os: "Arch Linux 6.17".into(),
                cpu_model: "Intel(R) Core(TM) i7-2620M CPU @ 2.70GHz".into(),
                cpu_cores: 2,
                cpu_threads: 4,
                ram_bytes: 7_846 * 1024 * 1024,
            },
            storage: StorageFacts {
                data_dir: "/home/mrcj/cyxwiz/node-data".into(),
                offered_bytes: 700 << 30,
                used_bytes: 6_500_000,
                chunks: 42,
                disk_free_bytes: 830 << 30,
                disk_total_bytes: 915 << 30,
            },
            server: ServerFacts {
                address: "http://192.168.1.116:50060".into(),
                registered: true,
                last_heartbeat_unix: 1_800_000_030,
                last_error: String::new(),
            },
        };

        let json = serde_json::to_string(&report).unwrap();
        let back: NodeStatusReport = serde_json::from_str(&json).unwrap();

        assert_eq!(back, report);
        assert!(!json.to_lowercase().contains("secret"));
        assert!(!json.to_lowercase().contains("token"));
    }

    #[test]
    fn an_older_node_without_some_fields_still_parses() {
        let back: NodeStatusReport =
            serde_json::from_str(r#"{"node_id":"abc","server":{"registered":true}}"#).unwrap();
        assert_eq!(back.node_id, "abc");
        assert!(back.server.registered);
        assert_eq!(back.storage.chunks, 0);
    }
}
