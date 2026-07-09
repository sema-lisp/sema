//! Native OAuth 2.1 client for authenticated remote (HTTP) MCP servers.
//!
//! Implements the MCP authorization spec (`2025-11-25`) — an OAuth 2.1
//! Authorization-Code + PKCE flow over an RFC 8252 loopback redirect, with a
//! device-authorization (RFC 8628) alternative and a print-URL floor for
//! headless boxes. The MCP server is only the OAuth *resource server*; a
//! separate *authorization server* (discovered via RFC 9728 / RFC 8414) issues
//! tokens. See `docs/plans/2026-06-21-mcp-client-spike.md`.
//!
//! Design (Option B, hand-roll + `oauth2`): the `oauth2` crate supplies the
//! security-sensitive PKCE and CSRF primitives; the plain form-encoded token /
//! discovery / registration HTTP is driven directly with our own reqwest client
//! so nothing depends on `oauth2`'s (reqwest-0.12) built-in client.

pub mod device;
pub mod discovery;
pub mod flow;
pub mod login;
pub mod loopback;
pub mod scoped;
pub mod store;

use oauth2::{CsrfToken, PkceCodeChallenge, PkceCodeVerifier};

/// A freshly generated PKCE pair plus a CSRF `state`, produced by the `oauth2`
/// crate so we never hand-roll the challenge derivation.
pub struct PkceSession {
    pub challenge: String,
    pub challenge_method: String,
    pub verifier: String,
    pub state: String,
}

/// Generate a PKCE S256 challenge/verifier and a random CSRF `state`.
pub fn new_pkce_session() -> PkceSession {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    PkceSession {
        challenge: challenge.as_str().to_string(),
        // `new_random_sha256` always yields the S256 method.
        challenge_method: "S256".to_string(),
        verifier: verifier.secret().to_string(),
        state: CsrfToken::new_random().secret().to_string(),
    }
}

/// Re-wrap a stored verifier string (kept only for the life of one in-flight
/// login) as the `oauth2` newtype, for symmetry with `new_pkce_session`.
pub fn verifier(secret: impl Into<String>) -> PkceCodeVerifier {
    PkceCodeVerifier::new(secret.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_session_is_well_formed() {
        let s = new_pkce_session();
        assert_eq!(s.challenge_method, "S256");
        // RFC 7636: the verifier is 43–128 chars from the unreserved set.
        assert!(
            (43..=128).contains(&s.verifier.len()),
            "verifier len {}",
            s.verifier.len()
        );
        assert!(!s.challenge.is_empty());
        assert!(!s.state.is_empty());
        // Two sessions must not collide (random verifier + state).
        let t = new_pkce_session();
        assert_ne!(s.verifier, t.verifier);
        assert_ne!(s.state, t.state);
    }
}
