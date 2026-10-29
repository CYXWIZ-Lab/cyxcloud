//! The sign-in check at the door: a request that carries a token must carry
//! a good one. A request without a token goes through and the service
//! decides (sign-in, node registration and dataset streams need none), so
//! the door keeps no second list of public methods that could drift from
//! the services'. The services verify again.

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

/// Secrets that sign the tokens clients send: the shared user secret
/// (`JWT_SECRET`: the API, central and the gateway) and the admin secret.
#[derive(Clone)]
pub struct TokenCheck {
    keys: Vec<DecodingKey>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `Authorization` is there but is not `Bearer <token>`
    Malformed,
    /// Bad signature, expired, or not a JWT
    Invalid,
}

impl Refusal {
    pub fn message(&self) -> &'static str {
        match self {
            Refusal::Malformed => "the authorization header is not `Bearer <token>`",
            Refusal::Invalid => "the sign-in is not valid or has expired",
        }
    }
}

impl TokenCheck {
    pub fn new(secrets: &[String]) -> anyhow::Result<Self> {
        let keys: Vec<DecodingKey> = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| DecodingKey::from_secret(s.as_bytes()))
            .collect();
        if keys.is_empty() {
            anyhow::bail!("no JWT secret: set JWT_SECRET (and ADMIN_JWT_SECRET for admin tokens)");
        }
        Ok(Self { keys })
    }

    /// `Ok(None)`: no token; `Ok(Some(account))`: a good token and the
    /// account it names (its `sub`, for the audit log).
    pub fn check(&self, authorization: Option<&str>) -> Result<Option<String>, Refusal> {
        let Some(value) = authorization else {
            return Ok(None);
        };
        let token = value
            .strip_prefix("Bearer ")
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(Refusal::Malformed)?;
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_aud = false;
        validation.set_required_spec_claims(&["exp"]);
        for key in &self.keys {
            if let Ok(data) = decode::<serde_json::Value>(token, key, &validation) {
                let sub = data
                    .claims
                    .get("sub")
                    .and_then(|s| s.as_str())
                    .unwrap_or("-")
                    .to_string();
                return Ok(Some(sub));
            }
        }
        Err(Refusal::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    fn token(secret: &str, exp_offset: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims =
            serde_json::json!({ "sub": "6ac29f91", "exp": now + exp_offset, "iss": "cyxwiz-api" });
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    fn check() -> TokenCheck {
        TokenCheck::new(&["user-secret".into(), "admin-secret".into()]).unwrap()
    }

    #[test]
    fn no_token_is_for_the_service_to_judge() {
        assert_eq!(check().check(None), Ok(None));
    }

    #[test]
    fn a_good_token_names_its_account() {
        let t = format!("Bearer {}", token("user-secret", 300));
        assert_eq!(check().check(Some(&t)), Ok(Some("6ac29f91".into())));
        let admin = format!("Bearer {}", token("admin-secret", 300));
        assert!(check().check(Some(&admin)).unwrap().is_some());
    }

    #[test]
    fn a_bad_token_stops_at_the_door() {
        let other = format!("Bearer {}", token("someone-else", 300));
        assert_eq!(check().check(Some(&other)), Err(Refusal::Invalid));
        let expired = format!("Bearer {}", token("user-secret", -3600));
        assert_eq!(check().check(Some(&expired)), Err(Refusal::Invalid));
        assert_eq!(check().check(Some("Bearer ")), Err(Refusal::Malformed));
        assert_eq!(check().check(Some("Basic abc")), Err(Refusal::Malformed));
        assert_eq!(
            check().check(Some("Bearer not-a-jwt")),
            Err(Refusal::Invalid)
        );
    }

    #[test]
    fn the_door_needs_a_secret() {
        assert!(TokenCheck::new(&[String::new()]).is_err());
    }
}
