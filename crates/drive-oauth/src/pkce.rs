//! PKCE verifier/challenge and CSRF state, per RFC 7636 with the S256 method.

use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::error::Result;

/// 32 random bytes base64url-encode to 43 characters, the minimum RFC 7636 allows.
const ENTROPY_BYTES: usize = 32;

/// The `code_verifier` for one authorization request; single use, never persisted.
#[derive(Clone)]
pub struct CodeVerifier(String);

impl CodeVerifier {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; ENTROPY_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// The RFC 7636 test vector, exposed so tests can pin the S256 implementation.
    pub fn from_raw(raw: &str) -> Self {
        Self(raw.to_string())
    }

    pub fn verifier(&self) -> &str {
        &self.0
    }

    /// `BASE64URL(SHA256(ASCII(code_verifier)))`, no padding.
    pub fn challenge(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

// A verifier can be spent at the token endpoint, so it is never printed.
impl fmt::Debug for CodeVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodeVerifier(<redacted>)")
    }
}

/// CSRF nonce echoed back on the redirect. Public: it travels in the URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State(String);

impl State {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; ENTROPY_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub fn from_raw(raw: &str) -> Self {
        Self(raw.to_string())
    }

    pub fn state(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 appendix B: the verifier below must produce this challenge.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn matches_the_rfc_7636_test_vector() {
        let verifier = CodeVerifier::from_raw(RFC_VERIFIER);
        assert_eq!(verifier.challenge(), RFC_CHALLENGE);
    }

    #[test]
    fn generated_verifier_is_43_url_safe_characters() {
        let verifier = CodeVerifier::generate().expect("entropy");
        assert_eq!(verifier.verifier().len(), 43);
        assert!(
            verifier
                .verifier()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "verifier must be base64url without padding: {}",
            verifier.verifier()
        );
    }

    #[test]
    fn generated_challenge_is_43_url_safe_characters() {
        let challenge = CodeVerifier::generate().expect("entropy").challenge();
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }

    #[test]
    fn generated_values_differ() {
        assert_ne!(
            CodeVerifier::generate().expect("a").verifier(),
            CodeVerifier::generate().expect("b").verifier()
        );
        assert_ne!(State::generate().expect("a").state(), State::generate().expect("b").state());
    }

    #[test]
    fn state_is_43_characters() {
        let state = State::generate().expect("entropy");
        assert_eq!(state.state().len(), 43);
        assert_eq!(State::from_raw(RFC_VERIFIER).state(), RFC_VERIFIER);
    }

    #[test]
    fn debug_does_not_leak_the_verifier() {
        let verifier = CodeVerifier::from_raw(RFC_VERIFIER);
        assert!(!format!("{verifier:?}").contains(RFC_VERIFIER));
    }
}
