//! Refresh tokens live in the OS keychain and nowhere else — never SQLite, never a
//! config file (D5, issue #25).

use keyring::Entry;

use crate::error::Result;

/// Keyring service name; the account id is the key, so accounts stay independent.
const SERVICE: &str = "9Drive Desktop";

pub fn store_refresh_token(account: &str, token: &str) -> Result<()> {
    entry(account)?.set_password(token)?;
    Ok(())
}

pub fn read_refresh_token(account: &str) -> Result<String> {
    Ok(entry(account)?.get_password()?)
}

/// Removes the entry; the caller does this even when the revoke call fails (issue #28).
pub fn delete_refresh_token(account: &str) -> Result<()> {
    entry(account)?.delete_credential()?;
    Ok(())
}

/// False when the platform has no usable credential store, so Settings can warn
/// instead of failing on the first connect.
pub fn is_available() -> bool {
    Entry::store_status().is_ok()
}

fn entry(account: &str) -> Result<Entry> {
    Ok(Entry::new(SERVICE, account)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distinct per test, so the two tests cannot race on one keyring entry.
    const ROUND_TRIP_ACCOUNT: &str = "drive-oauth-self-test-rt@example.invalid";
    const MISSING_ACCOUNT: &str = "drive-oauth-self-test-absent@example.invalid";

    #[test]
    fn store_read_delete_round_trip() {
        if !is_available() {
            eprintln!("skipping keychain round-trip: this platform has no credential store");
            return;
        }
        if let Err(err) = store_refresh_token(ROUND_TRIP_ACCOUNT, "self-test-refresh-token") {
            eprintln!("skipping keychain round-trip: {err}");
            return;
        }

        assert_eq!(
            read_refresh_token(ROUND_TRIP_ACCOUNT).expect("read"),
            "self-test-refresh-token"
        );

        delete_refresh_token(ROUND_TRIP_ACCOUNT).expect("delete");
        assert!(read_refresh_token(ROUND_TRIP_ACCOUNT).is_err(), "entry must be gone");
    }

    #[test]
    fn errors_are_translated_not_leaked_as_keyring_types() {
        if !is_available() {
            eprintln!("skipping keychain error test: this platform has no credential store");
            return;
        }
        match read_refresh_token(MISSING_ACCOUNT) {
            Ok(_) => panic!("an account that was never stored must not resolve"),
            Err(err) => assert_eq!(err.code(), "keychain"),
        }
    }
}
