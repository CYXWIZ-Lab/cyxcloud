//! Reading a JWT's claims without verifying it. The CLI and the storage node
//! use this only to know when a sign-in has run out, never to trust it: the
//! service that issued the token verifies it.

use base64::Engine as _;

/// The `exp` claim (Unix seconds) of a JWT.
pub fn expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_i64()
}

/// Whether a token has run out at `now` (Unix seconds). A token whose expiry
/// cannot be read counts as run out.
pub fn has_expired(token: &str, now: i64) -> bool {
    expiry(token).is_none_or(|exp| exp <= now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(body: &str) -> String {
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", e.encode(br#"{"alg":"HS256"}"#), e.encode(body))
    }

    #[test]
    fn the_expiry_is_read_from_the_token() {
        let t = token(r#"{"sub":"6ac29f910171abdbcfdf8a04","exp":1800086400}"#);
        assert_eq!(expiry(&t), Some(1_800_086_400));
        assert_eq!(expiry("not-a-jwt"), None);
    }

    #[test]
    fn a_token_without_a_readable_expiry_has_run_out() {
        let t = token(r#"{"exp":1000}"#);
        assert!(!has_expired(&t, 999));
        assert!(has_expired(&t, 1000));
        assert!(has_expired("not-a-jwt", 0));
        assert!(has_expired(&token(r#"{"sub":"x"}"#), 0));
    }
}
