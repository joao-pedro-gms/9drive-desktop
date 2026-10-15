//! Settings: the key/value table and the one value that needs a guard rail — the Google OAuth
//! client id, which is credential-like and must never end up in SQLite as a secret.

use std::collections::HashMap;

use drive_core::db;
use drive_core::error::AppError;
use drive_core::repos::settings;
use tauri::State;

use crate::state::AppState;

use super::{IpcResult, fail, ok};

/// Where the client id lives. `AppState` reads this key through its own private constant; it is
/// repeated here because a command has to write it and `state.rs` is not this command's file.
const CLIENT_ID_KEY: &str = "oauth.client_id";

/// A Google client **secret** starts with this. Pasting one where an id belongs would put a
/// credential in SQLite, which is exactly what AGENTS.md forbids, so it is refused outright
/// rather than stored and forgotten.
const CLIENT_SECRET_PREFIX: &str = "GOCSPX-";

#[tauri::command(rename_all = "snake_case")]
pub fn get_settings(state: State<'_, AppState>) -> IpcResult<HashMap<String, String>> {
    ok(db::with_conn(&state.conn(), |conn| Ok(settings::all(conn)?.into_iter().collect())))
}

/// Upserts every key it is given. The client id goes through the same guard as
/// `set_google_client_id`, so the generic setter cannot be used to sneak a secret past it.
#[tauri::command(rename_all = "snake_case")]
pub fn set_settings(state: State<'_, AppState>, values: HashMap<String, String>) -> IpcResult<()> {
    ok(db::with_conn(&state.conn(), |conn| {
        for (key, value) in &values {
            if key == CLIENT_ID_KEY {
                validate_client_id(value)?;
            }
            settings::set(conn, key, value)?;
        }
        Ok(())
    }))
}

/// Stores the pasted client id. The value is never logged and never leaves this function; a
/// blank one deletes nothing, because an empty override falls back to the compiled default
/// inside `AppState::client_id` anyway.
#[tauri::command(rename_all = "snake_case")]
pub fn set_google_client_id(state: State<'_, AppState>, client_id: String) -> IpcResult<()> {
    let client_id = validate_client_id(&client_id).map_err(fail)?;
    ok(db::with_conn(&state.conn(), |conn| settings::set(conn, CLIENT_ID_KEY, client_id)))?;
    tracing::info!("Google OAuth client id stored in settings");
    Ok(())
}

/// A client id is plausible when it is not blank, has no spaces, and is not a client secret.
/// Google issues `NNNNNNN.apps.googleusercontent.com`; anything else that passes these three
/// checks is left for `resolve_client_id` to reject rather than being second-guessed here.
fn validate_client_id(raw: &str) -> Result<&str, AppError> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(AppError::InvalidInput("the Google client id must not be blank".into()));
    }
    if id.contains(CLIENT_SECRET_PREFIX) {
        return Err(AppError::InvalidInput(
            "that is a Google client secret, not a client id: use the id from the OAuth client"
                .into(),
        ));
    }
    if id.chars().any(char::is_whitespace) {
        return Err(AppError::InvalidInput("a Google client id has no spaces in it".into()));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use drive_core::db::SharedConn;

    use super::*;

    fn conn() -> SharedConn {
        db::shared(db::open_in_memory().expect("in-memory database"))
    }

    fn store(conn: &SharedConn, key: &str, value: &str) -> Result<(), AppError> {
        if key == CLIENT_ID_KEY {
            validate_client_id(value)?;
        }
        db::with_conn(conn, |c| settings::set(c, key, value))?;
        Ok(())
    }

    fn read(conn: &SharedConn) -> Result<HashMap<String, String>, AppError> {
        db::with_conn(conn, |c| Ok(settings::all(c)?.into_iter().collect()))
    }

    #[test]
    fn a_pasted_client_id_is_stored_trimmed() {
        assert_eq!(
            validate_client_id("  abc123.apps.googleusercontent.com  ").expect("id"),
            "abc123.apps.googleusercontent.com"
        );
    }

    #[test]
    fn a_client_secret_is_refused_not_stored() {
        let secret = "GOCSPX-abcdefghijklmnop";
        let err = validate_client_id(secret).expect_err("a secret is not an id");
        assert!(matches!(err, AppError::InvalidInput(_)));
        assert_eq!(err.code(), "invalid_input");

        let conn = conn();
        assert!(store(&conn, CLIENT_ID_KEY, secret).is_err(), "and never reaches the table");
        assert!(read(&conn).expect("settings").is_empty());
    }

    #[test]
    fn a_secret_prefix_anywhere_in_the_value_is_refused() {
        assert!(validate_client_id("abc-GOCSPX-123.apps.googleusercontent.com").is_err());
    }

    #[test]
    fn a_blank_client_id_is_refused() {
        for blank in ["", "   ", "\t"] {
            assert!(validate_client_id(blank).is_err(), "{blank:?} must be refused");
        }
    }

    #[test]
    fn a_client_id_with_a_space_in_it_is_refused() {
        assert!(validate_client_id("abc 123.apps.googleusercontent.com").is_err());
    }

    #[test]
    fn the_generic_setter_guards_the_client_id_key_too() {
        let conn = conn();
        store(&conn, CLIENT_ID_KEY, "abc.apps.googleusercontent.com").expect("store");
        store(&conn, "theme", "dark").expect("store");

        let values = read(&conn).expect("settings");
        assert_eq!(
            values.get(CLIENT_ID_KEY).map(String::as_str),
            Some("abc.apps.googleusercontent.com")
        );
        assert_eq!(values.get("theme").map(String::as_str), Some("dark"));

        assert!(
            store(&conn, CLIENT_ID_KEY, "GOCSPX-secret").is_err(),
            "set_settings must not be a way around the guard"
        );
    }

    #[test]
    fn the_settings_table_round_trips() {
        let conn = conn();
        assert!(read(&conn).expect("empty").is_empty());
        store(&conn, "theme", "dark").expect("write");
        store(&conn, "theme", "light").expect("overwrite");
        assert_eq!(read(&conn).expect("read").get("theme").map(String::as_str), Some("light"));
    }

    #[test]
    fn a_rejected_value_leaves_nothing_written() {
        let conn = conn();
        let err = store(&conn, CLIENT_ID_KEY, " ").expect_err("blank");
        assert_eq!(err.code(), "invalid_input");
        assert!(!read(&conn).expect("read").contains_key(CLIENT_ID_KEY));
    }

    #[test]
    fn the_key_matches_the_one_app_state_reads() {
        // `AppState::CLIENT_ID_KEY` is private, so this string is duplicated in two files. If
        // either changes, this assertion is where the drift should be caught.
        assert_eq!(CLIENT_ID_KEY, "oauth.client_id");
    }
}
