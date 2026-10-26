//! A storage node's identity: an Ed25519 key the node holds, a node id derived
//! from its public key, and a signed proof of registration.
//!
//! The gateway believes a node id only when the registration is signed by the
//! key the id was derived from (and, for a known node, by the key it was
//! registered with). Nobody can claim or take over a node id without the key
//! (TOFIX176 P2).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::path::Path;

/// Domain separation for registration signatures: a signature over these
/// bytes can never be mistaken for anything else a node signs.
const REGISTRATION_DOMAIN: &[u8] = b"cyxcloud-node-register-v1\0";

/// How far a registration timestamp may be from the server clock.
pub const REGISTRATION_MAX_SKEW_SECS: i64 = 300;

#[derive(Debug, thiserror::Error)]
pub enum NodeIdentityError {
    #[error("node key file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("node key file {0} does not hold a 32-byte Ed25519 seed (hex)")]
    MalformedKey(String),
    #[error("public key is not a valid Ed25519 key")]
    MalformedPublicKey,
    #[error("signature is malformed")]
    MalformedSignature,
    #[error("node id {node_id} is not derived from the presented public key")]
    IdNotFromKey { node_id: String },
    #[error("registration timestamp is {skew_secs} s from now (limit {limit_secs} s)")]
    StaleTimestamp { skew_secs: i64, limit_secs: i64 },
    #[error("registration signature does not verify")]
    BadSignature,
}

/// The node's private key, kept on its disk.
pub struct NodeKey {
    signing: SigningKey,
}

impl NodeKey {
    /// A fresh random key.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// From a 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Load the key at `path`, or generate one and save it there (owner-only). The file holds the 32-byte seed as 64 hex characters.
    pub fn load_or_generate(path: &Path) -> Result<Self, NodeIdentityError> {
        if path.exists() {
            return Self::load(path);
        }
        let key = Self::generate();
        key.save(path)?;
        Ok(key)
    }

    /// Load the key at `path`.
    pub fn load(path: &Path) -> Result<Self, NodeIdentityError> {
        let text = std::fs::read_to_string(path).map_err(|source| NodeIdentityError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let bytes = decode_hex(text.trim())
            .ok_or_else(|| NodeIdentityError::MalformedKey(path.display().to_string()))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| NodeIdentityError::MalformedKey(path.display().to_string()))?;
        Ok(Self::from_seed(&seed))
    }

    /// Save the seed at `path` as hex, readable by the owner only (every OS).
    pub fn save(&self, path: &Path) -> Result<(), NodeIdentityError> {
        crate::owner_only::write_owner_only(path, encode_hex(self.signing.to_bytes().as_ref()))
            .map_err(|source| NodeIdentityError::Io {
                path: path.display().to_string(),
                source,
            })
    }

    /// The public key, 32 bytes.
    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// The public key as hex (what goes on the wire and into the nodes table).
    pub fn public_key_hex(&self) -> String {
        encode_hex(&self.public_key())
    }

    /// The node id this key owns.
    pub fn node_id(&self) -> String {
        node_id_from_public_key(&self.public_key())
    }

    /// Sign a registration of `listen_addr` at `timestamp` (Unix seconds).
    pub fn sign_registration(&self, listen_addr: &str, timestamp: i64) -> Vec<u8> {
        let message = registration_message(&self.node_id(), listen_addr, timestamp);
        self.signing.sign(&message).to_bytes().to_vec()
    }
}

/// The node id is the first 16 bytes of BLAKE3(public key), as 32 hex
/// characters: short enough to read, long enough never to collide.
pub fn node_id_from_public_key(public_key: &[u8]) -> String {
    let hash = blake3::hash(public_key);
    encode_hex(&hash.as_bytes()[..16])
}

fn registration_message(node_id: &str, listen_addr: &str, timestamp: i64) -> Vec<u8> {
    let mut m = Vec::with_capacity(REGISTRATION_DOMAIN.len() + node_id.len() + listen_addr.len() + 10);
    m.extend_from_slice(REGISTRATION_DOMAIN);
    m.extend_from_slice(node_id.as_bytes());
    m.push(0);
    m.extend_from_slice(listen_addr.as_bytes());
    m.push(0);
    m.extend_from_slice(&timestamp.to_be_bytes());
    m
}

/// What a gateway checks before it believes a registration.
pub struct RegistrationProof<'a> {
    pub node_id: &'a str,
    pub public_key_hex: &'a str,
    pub listen_addr: &'a str,
    pub timestamp: i64,
    pub signature: &'a [u8],
}

impl RegistrationProof<'_> {
    /// Verify the proof against `now` (Unix seconds). Checks, in order: the
    /// public key parses, the node id is derived from it, the timestamp is
    /// within the skew limit, the signature verifies.
    pub fn verify(&self, now: i64) -> Result<VerifyingKey, NodeIdentityError> {
        let key_bytes =
            decode_hex(self.public_key_hex).ok_or(NodeIdentityError::MalformedPublicKey)?;
        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| NodeIdentityError::MalformedPublicKey)?;
        let key = VerifyingKey::from_bytes(&key_array)
            .map_err(|_| NodeIdentityError::MalformedPublicKey)?;

        if node_id_from_public_key(&key_array) != self.node_id {
            return Err(NodeIdentityError::IdNotFromKey {
                node_id: self.node_id.to_string(),
            });
        }

        let skew = (now - self.timestamp).abs();
        if skew > REGISTRATION_MAX_SKEW_SECS {
            return Err(NodeIdentityError::StaleTimestamp {
                skew_secs: skew,
                limit_secs: REGISTRATION_MAX_SKEW_SECS,
            });
        }

        let signature =
            Signature::from_slice(self.signature).map_err(|_| NodeIdentityError::MalformedSignature)?;
        let message = registration_message(self.node_id, self.listen_addr, self.timestamp);
        key.verify(&message, &signature)
            .map_err(|_| NodeIdentityError::BadSignature)?;
        Ok(key)
    }
}

/// Load the node's access secret at `path`, or make one (32 random bytes as
/// hex, owner-only). The node hands it to the gateway at registration
/// and accepts chunk RPCs that present it.
pub fn load_or_generate_access_secret(path: &Path) -> Result<String, NodeIdentityError> {
    let io = |source| NodeIdentityError::Io {
        path: path.display().to_string(),
        source,
    };
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(io)?;
        let secret = text.trim().to_string();
        if secret.len() != 64 || decode_hex(&secret).is_none() {
            return Err(NodeIdentityError::MalformedKey(path.display().to_string()));
        }
        return Ok(secret);
    }
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    let secret = encode_hex(&bytes);
    crate::owner_only::write_owner_only(path, &secret).map_err(io)?;
    Ok(secret)
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() & 1 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "192.168.1.180:50051";

    #[test]
    fn a_signed_registration_verifies() {
        let key = NodeKey::generate();
        let now = 1_800_000_000;
        let sig = key.sign_registration(ADDR, now);
        let pk = key.public_key_hex();
        let node_id = key.node_id();

        let proof = RegistrationProof {
            node_id: &node_id,
            public_key_hex: &pk,
            listen_addr: ADDR,
            timestamp: now,
            signature: &sig,
        };

        assert!(proof.verify(now + 30).is_ok());
        assert_eq!(node_id.len(), 32);
    }

    #[test]
    fn a_node_id_not_derived_from_the_key_is_refused() {
        let key = NodeKey::generate();
        let now = 1_800_000_000;
        let sig = key.sign_registration(ADDR, now);
        let pk = key.public_key_hex();
        let stolen = NodeKey::generate().node_id(); // somebody else's id

        let proof = RegistrationProof {
            node_id: &stolen,
            public_key_hex: &pk,
            listen_addr: ADDR,
            timestamp: now,
            signature: &sig,
        };

        assert!(matches!(
            proof.verify(now),
            Err(NodeIdentityError::IdNotFromKey { .. })
        ));
    }

    #[test]
    fn a_changed_address_or_time_breaks_the_signature() {
        let key = NodeKey::generate();
        let now = 1_800_000_000;
        let sig = key.sign_registration(ADDR, now);
        let pk = key.public_key_hex();
        let node_id = key.node_id();

        let other_addr = RegistrationProof {
            node_id: &node_id,
            public_key_hex: &pk,
            listen_addr: "10.0.0.9:50051",
            timestamp: now,
            signature: &sig,
        };
        assert!(matches!(other_addr.verify(now), Err(NodeIdentityError::BadSignature)));

        let other_time = RegistrationProof {
            node_id: &node_id,
            public_key_hex: &pk,
            listen_addr: ADDR,
            timestamp: now + 1,
            signature: &sig,
        };
        assert!(matches!(other_time.verify(now), Err(NodeIdentityError::BadSignature)));
    }

    #[test]
    fn an_old_registration_cannot_be_replayed() {
        let key = NodeKey::generate();
        let then = 1_800_000_000;
        let sig = key.sign_registration(ADDR, then);
        let pk = key.public_key_hex();
        let node_id = key.node_id();

        let proof = RegistrationProof {
            node_id: &node_id,
            public_key_hex: &pk,
            listen_addr: ADDR,
            timestamp: then,
            signature: &sig,
        };

        assert!(matches!(
            proof.verify(then + REGISTRATION_MAX_SKEW_SECS + 1),
            Err(NodeIdentityError::StaleTimestamp { .. })
        ));
    }

    #[test]
    fn another_key_cannot_sign_for_this_node() {
        let key = NodeKey::generate();
        let impostor = NodeKey::generate();
        let now = 1_800_000_000;
        let node_id = key.node_id();
        let pk = key.public_key_hex();
        // the impostor signs the victim's node id with its own key
        let message = registration_message(&node_id, ADDR, now);
        let sig = impostor.signing.sign(&message).to_bytes().to_vec();

        let proof = RegistrationProof {
            node_id: &node_id,
            public_key_hex: &pk,
            listen_addr: ADDR,
            timestamp: now,
            signature: &sig,
        };

        assert!(matches!(proof.verify(now), Err(NodeIdentityError::BadSignature)));
    }

    #[test]
    fn the_key_file_round_trips_and_keeps_the_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("node").join("key");

        let first = NodeKey::load_or_generate(&path).unwrap();
        let second = NodeKey::load_or_generate(&path).unwrap();

        assert_eq!(first.node_id(), second.node_id());
        assert_eq!(first.public_key(), second.public_key());
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn a_damaged_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("key");
        std::fs::write(&path, "not hex at all").unwrap();

        assert!(matches!(
            NodeKey::load_or_generate(&path),
            Err(NodeIdentityError::MalformedKey(_))
        ));
    }

    #[test]
    fn the_access_secret_is_made_once_and_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("node").join("access_secret");

        let a = load_or_generate_access_secret(&path).unwrap();
        let b = load_or_generate_access_secret(&path).unwrap();

        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(decode_hex(&a).is_some());
    }

    #[test]
    fn hex_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_hex(&encode_hex(&bytes)).unwrap(), bytes);
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }
}
