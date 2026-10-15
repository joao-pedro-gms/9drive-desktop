//! Upload routing: which connected account takes a given file.
//!
//! F4 of `docs/rewrite-plan.md` §8. The router is pure decision-making over quota rows already
//! in SQLite, so it holds no I/O of its own and needs no provider.

use std::fmt;
use std::str::FromStr;

use crate::db::{SharedConn, with_conn};
use crate::error::{AppError, Result};
use crate::models::Account;
use crate::repos::accounts;

/// Which account an upload goes to.
///
/// [`Display`] and [`FromStr`] round-trip because the setting is persisted as `TEXT`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Policy {
    /// The eligible account with the most free space.
    #[default]
    MostAvailable,
    /// The next eligible account in id order, wrapping, one call per account.
    RoundRobin,
    /// The first listed id that is eligible and fits. An empty list has no parseable spelling.
    Fixed(Vec<i64>),
}

impl fmt::Display for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MostAvailable => f.write_str("most_available"),
            Self::RoundRobin => f.write_str("round_robin"),
            Self::Fixed(ids) => {
                f.write_str("fixed:")?;
                for (i, id) in ids.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{id}")?;
                }
                Ok(())
            }
        }
    }
}

impl FromStr for Policy {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "most_available" => return Ok(Self::MostAvailable),
            "round_robin" => return Ok(Self::RoundRobin),
            _ => {}
        }
        let Some(list) = s.strip_prefix("fixed:") else {
            return Err(AppError::InvalidInput(format!("unknown routing policy {s:?}")));
        };
        if list.is_empty() {
            return Err(AppError::InvalidInput(
                "fixed policy needs at least one account id".into(),
            ));
        }
        let ids = list
            .split(',')
            .map(|part| part.trim().parse::<i64>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                AppError::InvalidInput(format!("routing policy {s:?} has a bad id: {e}"))
            })?;
        Ok(Self::Fixed(ids))
    }
}

/// One eligible account and the space it has left, in `u64`.
type Candidate = (Account, u64);

/// The account an upload goes to, plus how much room it had when it was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountChoice {
    pub account: Account,
    pub free_bytes: u64,
}

/// Holds the policy plus the round-robin cursor, so selecting takes `&mut self`.
///
/// `last_used_at` is deliberately not written here: the upload queue that owns "this account was
/// just used" runs after the choice is made and stamps it through `accounts::update`.
pub struct Router {
    policy: Policy,
    cursor: usize,
}

impl Router {
    pub fn new(policy: Policy) -> Self {
        Self { policy, cursor: 0 }
    }

    /// Picks the account a `size_bytes` upload goes to.
    pub fn select(&mut self, conn: &SharedConn, size_bytes: u64) -> Result<AccountChoice> {
        // One lock scope for both reads: a quota refresh landing between them would size the
        // candidate list against one set of numbers and rank it against another.
        let mut candidates = with_conn(conn, |c| {
            let mut found: Vec<Candidate> = Vec::new();
            for account in accounts::list(c)? {
                // No `9drive` folder yet, so there is nowhere to upload into.
                if account.drive_folder_id.is_none() {
                    continue;
                }
                match accounts::get_quota(c, account.id) {
                    Ok(quota) => {
                        found.push((account, free_bytes(quota.limit_bytes, quota.usage_bytes)))
                    }
                    // Never quota-synced: the room is unknown, so do not claim it fits.
                    Err(AppError::NotFound { .. }) => continue,
                    Err(err) => return Err(err),
                }
            }
            Ok(found)
        })?;
        // Rank on id, not on the row order `accounts::list` happens to return, so ties resolve
        // the same way every run.
        candidates.sort_by_key(|(account, _)| account.id);

        let len = candidates.len();
        let fits = |i: usize| candidates[i].1 >= size_bytes;
        let picked = match &self.policy {
            // `max_by` returns the last of several equal maxima, so the id is compared the other
            // way round to put the lowest id first.
            Policy::MostAvailable => (0..len).filter(|&i| fits(i)).max_by(|&a, &b| {
                candidates[a]
                    .1
                    .cmp(&candidates[b].1)
                    .then(candidates[b].0.id.cmp(&candidates[a].0.id))
            }),
            // Accounts that do not fit are stepped over, and the cursor is left past the choice.
            Policy::RoundRobin => {
                let start = self.cursor % len.max(1);
                (0..len).map(|k| (start + k) % len).find(|&i| fits(i))
            }
            Policy::Fixed(ids) => {
                ids.iter().find_map(|id| (0..len).find(|&i| candidates[i].0.id == *id && fits(i)))
            }
        };

        let Some(index) = picked else {
            return Err(AppError::Conflict(match candidates.iter().map(|(_, f)| *f).max() {
                // The UI has to show the user why, so name both numbers it already holds.
                Some(bytes) => format!(
                    "no account has room for {size_bytes} bytes: the most free space available is {bytes} bytes"
                ),
                None => format!(
                    "no connected account is initialised for uploads, so none can take {size_bytes} bytes"
                ),
            }));
        };
        if matches!(self.policy, Policy::RoundRobin) {
            self.cursor = index + 1;
        }
        let (account, free_bytes) = candidates.remove(index);
        Ok(AccountChoice { account, free_bytes })
    }
}

fn free_bytes(limit_bytes: i64, usage_bytes: i64) -> u64 {
    // Drive reports `limit` as `"0"` for an account with unlimited storage, so zero means
    // unlimited rather than "no space" -- every such account would otherwise be rejected.
    if limit_bytes == 0 {
        return u64::MAX;
    }
    // Usage can sit above the limit while trash is pending: clamp instead of wrapping negative.
    limit_bytes.saturating_sub(usage_bytes).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{shared, test_conn};
    use crate::models::AccountQuota;
    use rusqlite::{Connection, params};

    const GB: i64 = 1_000_000_000;

    /// `accounts::create` leaves `drive_folder_id` NULL, which is what "not initialised" means.
    fn account(conn: &Connection, email: &str, initialised: bool) -> i64 {
        let account = accounts::create(conn, "google", email, email).unwrap();
        if initialised {
            let folder = format!("folder-{}", account.id);
            conn.execute(
                "UPDATE accounts SET drive_folder_id = ?2 WHERE id = ?1",
                params![account.id, folder],
            )
            .unwrap();
        }
        account.id
    }

    fn quota(conn: &Connection, account_id: i64, limit: i64, usage: i64) {
        accounts::upsert_quota(
            conn,
            &AccountQuota {
                account_id,
                limit_bytes: limit,
                usage_bytes: usage,
                usage_in_drive: Some(usage),
                usage_in_drive_trash: Some(0),
                synced_at: 0,
            },
        )
        .unwrap();
    }

    /// An initialised account with `free` bytes free.
    fn with_free(conn: &Connection, email: &str, free: i64) -> i64 {
        let id = account(conn, email, true);
        quota(conn, id, 100 * GB, 100 * GB - free);
        id
    }

    #[test]
    fn default_policy_is_most_available_and_picks_the_roomiest() {
        let raw = test_conn();
        let roomy = with_free(&raw, "roomy@example.com", 80 * GB);
        let small = with_free(&raw, "small@example.com", 10 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::default());
        let chosen = router.select(&conn, 1).unwrap();
        assert_eq!(chosen.account.id, roomy);
        assert_eq!(chosen.free_bytes, (80 * GB) as u64);
        assert_ne!(chosen.account.id, small);
    }

    #[test]
    fn a_file_too_big_for_every_account_is_a_conflict_naming_both_numbers() {
        let raw = test_conn();
        with_free(&raw, "a@example.com", 20 * GB);
        with_free(&raw, "b@example.com", 30 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        let err = router.select(&conn, 45 * GB as u64).unwrap_err();
        assert_eq!(err.code(), "conflict");
        let message = err.to_string();
        assert!(message.contains(&format!("{}", 45 * GB)), "{message}");
        assert!(message.contains(&format!("{}", 30 * GB)), "{message}");
    }

    #[test]
    fn a_file_that_only_fits_the_second_account_lands_there() {
        let raw = test_conn();
        let tiny = with_free(&raw, "tiny@example.com", 5 * GB);
        let roomy = with_free(&raw, "roomy@example.com", 50 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        assert_eq!(router.select(&conn, 40 * GB as u64).unwrap().account.id, roomy);
        assert_ne!(roomy, tiny);
    }

    #[test]
    fn an_account_without_a_drive_folder_is_never_chosen() {
        let raw = test_conn();
        let uninitialised = account(&raw, "new@example.com", false);
        quota(&raw, uninitialised, 100 * GB, 0);
        let initialised = with_free(&raw, "ready@example.com", GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        assert_eq!(router.select(&conn, 1).unwrap().account.id, initialised);
        // Even with a policy that names it explicitly it stays out: there is no folder to use.
        let mut fixed = Router::new(Policy::Fixed(vec![uninitialised, initialised]));
        assert_eq!(fixed.select(&conn, 1).unwrap().account.id, initialised);
    }

    #[test]
    fn unlimited_storage_beats_a_nearly_full_account() {
        let raw = test_conn();
        let nearly_full = with_free(&raw, "full@example.com", 1);
        let unlimited = account(&raw, "unlimited@example.com", true);
        quota(&raw, unlimited, 0, 900 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        let chosen = router.select(&conn, 500 * GB as u64).unwrap();
        assert_eq!(chosen.account.id, unlimited);
        assert_eq!(chosen.free_bytes, u64::MAX);
        assert_ne!(chosen.account.id, nearly_full);
    }

    #[test]
    fn round_robin_alternates_and_advances_past_the_chosen_account() {
        let raw = test_conn();
        let first = with_free(&raw, "a@example.com", 50 * GB);
        let second = with_free(&raw, "b@example.com", 40 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::RoundRobin);
        let picked: Vec<i64> =
            (0..4).map(|_| router.select(&conn, 1).unwrap().account.id).collect();
        assert_eq!(picked, [first, second, first, second]);
    }

    #[test]
    fn round_robin_steps_over_an_account_that_does_not_fit() {
        let raw = test_conn();
        let small = with_free(&raw, "small@example.com", 2 * GB);
        let big = with_free(&raw, "big@example.com", 50 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::RoundRobin);
        assert_eq!(router.select(&conn, 10 * GB as u64).unwrap().account.id, big);
        assert_eq!(router.select(&conn, 10 * GB as u64).unwrap().account.id, big);
        assert_ne!(big, small);
    }

    #[test]
    fn fixed_takes_the_first_listed_id_that_fits() {
        let raw = test_conn();
        let first = with_free(&raw, "a@example.com", 10 * GB);
        let second = with_free(&raw, "b@example.com", 50 * GB);
        let third = with_free(&raw, "c@example.com", 40 * GB);
        let conn = shared(raw);

        // The listed id does not fit, so it falls through to the next listed one.
        let mut router = Router::new(Policy::Fixed(vec![first, second, third]));
        assert_eq!(router.select(&conn, 45 * GB as u64).unwrap().account.id, second);

        // An id that is not in `accounts` is ignored, not an error.
        let mut skipping = Router::new(Policy::Fixed(vec![9999, third]));
        assert_eq!(skipping.select(&conn, 1).unwrap().account.id, third);

        // Every listed id unusable: the roomiest account is still named in the message.
        let mut none = Router::new(Policy::Fixed(vec![9999]));
        assert_eq!(none.select(&conn, 1).unwrap_err().code(), "conflict");
    }

    #[test]
    fn ties_break_on_the_lower_account_id() {
        let raw = test_conn();
        let older = with_free(&raw, "older@example.com", 20 * GB);
        let newer = with_free(&raw, "newer@example.com", 20 * GB);
        assert!(older < newer);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        // Twice, so a result that depended on SQLite row order would show up.
        assert_eq!(router.select(&conn, 1).unwrap().account.id, older);
        assert_eq!(router.select(&conn, 1).unwrap().account.id, older);
    }

    #[test]
    fn usage_above_the_limit_clamps_to_zero_instead_of_panicking() {
        let raw = test_conn();
        let over = account(&raw, "over@example.com", true);
        quota(&raw, over, 10 * GB, 25 * GB);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        let err = router.select(&conn, 1).unwrap_err();
        assert_eq!(err.code(), "conflict");
        assert!(err.to_string().contains("0 bytes"), "{err}");
        // Exactly zero free is not claimed as unlimited either.
        assert_ne!(free_bytes(10 * GB, 25 * GB), u64::MAX);
    }

    #[test]
    fn an_account_without_a_quota_row_is_not_claimed_to_fit() {
        let raw = test_conn();
        account(&raw, "unsynced@example.com", true);
        let conn = shared(raw);

        let mut router = Router::new(Policy::MostAvailable);
        assert_eq!(router.select(&conn, 1).unwrap_err().code(), "conflict");
    }

    #[test]
    fn policy_strings_round_trip_and_bad_ones_are_invalid_input() {
        let parsed: Vec<Policy> =
            ["most_available", "round_robin", "fixed:1", "fixed:1,3", "fixed: 7 , 2 "]
                .iter()
                .map(|s| s.parse::<Policy>().unwrap())
                .collect();
        assert_eq!(
            parsed,
            [
                Policy::MostAvailable,
                Policy::RoundRobin,
                Policy::Fixed(vec![1]),
                Policy::Fixed(vec![1, 3]),
                Policy::Fixed(vec![7, 2]),
            ]
        );
        for policy in &parsed {
            assert_eq!(policy.to_string().parse::<Policy>().unwrap(), *policy);
        }

        for bad in ["", "nonsense", "mostavailable", "fixed:", "fixed:1,,2", "fixed:a", "fixed:1,x"]
        {
            let err = bad.parse::<Policy>().unwrap_err();
            assert_eq!(err.code(), "invalid_input", "{bad:?} gave {err}");
        }
    }
}
