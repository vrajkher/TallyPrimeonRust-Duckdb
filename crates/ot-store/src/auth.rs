//! Users, roles, sessions and API tokens in the book.
//!
//! The store keeps only hashes: password hashes come from the server, and session
//! and API tokens are stored as hashes of the secret the client holds.

use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{OptionalExt, Row, params};
use ot_core::auth::{Role, Scope};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    pub enabled: bool,
    pub roles: Vec<Role>,
    pub created_at: String,
}

/// A new user, with the password already hashed.
pub struct NewUserRecord {
    pub username: String,
    pub full_name: String,
    pub password_hash: String,
    pub roles: Vec<Role>,
}

/// Changes to a user; `None` leaves a field as it is.
#[derive(Default)]
pub struct UserUpdate {
    pub full_name: Option<String>,
    pub enabled: Option<bool>,
    pub roles: Option<Vec<Role>>,
    pub password_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub user: UserInfo,
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: String,
    pub revoked: bool,
}

const USER_COLUMNS: &str = "u.id, u.username, u.full_name, u.enabled, u.created_at::VARCHAR,
     coalesce(string_agg(r.role, ',' ORDER BY r.role), '')";

impl Store {
    pub fn user_count(&self) -> Result<i64, StoreError> {
        Ok(self
            .connection
            .query_row("SELECT count(*) FROM users", [], |row| row.get(0))?)
    }

    pub fn users(&self) -> Result<Vec<UserInfo>, StoreError> {
        let mut stmt = self.connection.prepare(&format!(
            "SELECT {USER_COLUMNS} FROM users u LEFT JOIN user_roles r ON r.user_id = u.id
             GROUP BY ALL ORDER BY u.username"
        ))?;
        Ok(stmt
            .query_map([], user_row)?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn user(&self, id: i64) -> Result<UserInfo, StoreError> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {USER_COLUMNS} FROM users u LEFT JOIN user_roles r ON r.user_id = u.id
                     WHERE u.id = ? GROUP BY ALL"
                ),
                [id],
                user_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::Missing("The user does not exist.".into()))
    }

    /// The user and password hash for a sign-in, if the user exists.
    pub fn credentials(&self, username: &str) -> Result<Option<(UserInfo, String)>, StoreError> {
        let found: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT id, password_hash FROM users WHERE username = ?",
                [username],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match found {
            Some((id, hash)) => Ok(Some((self.user(id)?, hash))),
            None => Ok(None),
        }
    }

    pub fn create_user(&mut self, user: NewUserRecord) -> Result<UserInfo, StoreError> {
        if user.roles.is_empty() {
            return Err(StoreError::Invalid(
                "Give the user at least one role.".into(),
            ));
        }
        let tx = self.connection.transaction()?;
        let taken: bool = tx.query_row(
            "SELECT count(*) > 0 FROM users WHERE username = ?",
            [&user.username],
            |row| row.get(0),
        )?;
        if taken {
            return Err(StoreError::Conflict(format!(
                "The user name {} is already taken.",
                user.username
            )));
        }
        let id: i64 = tx.query_row(
            "INSERT INTO users (username, full_name, password_hash) VALUES (?, ?, ?) RETURNING id",
            params![user.username, user.full_name, user.password_hash],
            |row| row.get(0),
        )?;
        for role in dedup(user.roles) {
            tx.execute(
                "INSERT INTO user_roles (user_id, role) VALUES (?, ?)",
                params![id, role.key()],
            )?;
        }
        tx.commit()?;
        self.user(id)
    }

    /// Updates a user. Disabling a user, or changing their password or roles, signs
    /// them out everywhere. The last enabled System Manager cannot be removed.
    pub fn update_user(&mut self, id: i64, update: UserUpdate) -> Result<UserInfo, StoreError> {
        let before = self.user(id)?;
        if let Some(roles) = &update.roles
            && roles.is_empty()
        {
            return Err(StoreError::Invalid(
                "Give the user at least one role.".into(),
            ));
        }
        let stays_admin = update.enabled.unwrap_or(before.enabled)
            && update
                .roles
                .as_ref()
                .unwrap_or(&before.roles)
                .contains(&Role::SystemManager);
        if !stays_admin && before.enabled && before.roles.contains(&Role::SystemManager) {
            let others: i64 = self.connection.query_row(
                "SELECT count(DISTINCT u.id) FROM users u JOIN user_roles r ON r.user_id = u.id
                 WHERE u.enabled AND r.role = 'system_manager' AND u.id <> ?",
                [id],
                |row| row.get(0),
            )?;
            if others == 0 {
                return Err(StoreError::Conflict(
                    "Keep at least one enabled System Manager.".into(),
                ));
            }
        }

        let sign_out = update.enabled == Some(false)
            || update.password_hash.is_some()
            || update.roles.is_some();
        let tx = self.connection.transaction()?;
        if let Some(name) = &update.full_name {
            tx.execute(
                "UPDATE users SET full_name = ? WHERE id = ?",
                params![name, id],
            )?;
        }
        if let Some(enabled) = update.enabled {
            tx.execute(
                "UPDATE users SET enabled = ? WHERE id = ?",
                params![enabled, id],
            )?;
        }
        if let Some(hash) = &update.password_hash {
            tx.execute(
                "UPDATE users SET password_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
        }
        if let Some(roles) = update.roles {
            tx.execute("DELETE FROM user_roles WHERE user_id = ?", [id])?;
            for role in dedup(roles) {
                tx.execute(
                    "INSERT INTO user_roles (user_id, role) VALUES (?, ?)",
                    params![id, role.key()],
                )?;
            }
        }
        if sign_out {
            tx.execute("DELETE FROM sessions WHERE user_id = ?", [id])?;
        }
        tx.commit()?;
        self.user(id)
    }

    pub fn create_session(
        &mut self,
        token_hash: &str,
        user_id: i64,
        csrf_token: &str,
        lifetime_seconds: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf_token, expires_unix)
             VALUES (?, ?, ?, ?)",
            params![
                token_hash,
                user_id,
                csrf_token,
                now_unix() + lifetime_seconds
            ],
        )?;
        Ok(())
    }

    /// A live session of an enabled user.
    pub fn session(&self, token_hash: &str) -> Result<Option<SessionRecord>, StoreError> {
        let found: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT s.user_id, s.csrf_token FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ? AND s.expires_unix > ? AND u.enabled",
                params![token_hash, now_unix()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match found {
            Some((user_id, csrf_token)) => Ok(Some(SessionRecord {
                user: self.user(user_id)?,
                csrf_token,
            })),
            None => Ok(None),
        }
    }

    pub fn delete_session(&mut self, token_hash: &str) -> Result<(), StoreError> {
        self.connection
            .execute("DELETE FROM sessions WHERE token_hash = ?", [token_hash])?;
        Ok(())
    }

    pub fn purge_expired_sessions(&mut self) -> Result<usize, StoreError> {
        Ok(self
            .connection
            .execute("DELETE FROM sessions WHERE expires_unix <= ?", [now_unix()])?)
    }

    pub fn create_token(
        &mut self,
        user_id: i64,
        name: &str,
        token_hash: &str,
        scope: Scope,
    ) -> Result<TokenInfo, StoreError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(StoreError::Invalid(
                "Name the token in 1 to 100 characters.".into(),
            ));
        }
        let id: i64 = self.connection.query_row(
            "INSERT INTO api_tokens (user_id, name, token_hash, scope) VALUES (?, ?, ?, ?) RETURNING id",
            params![user_id, name, token_hash, scope.key()],
            |row| row.get(0),
        )?;
        self.token_info(id)
    }

    /// Tokens of one user, or of everyone when `user_id` is `None`.
    pub fn tokens(&self, user_id: Option<i64>) -> Result<Vec<TokenInfo>, StoreError> {
        let mut stmt = self.connection.prepare(
            "SELECT t.id, t.user_id, u.username, t.name, t.scope, t.created_at::VARCHAR,
                    t.revoked_at IS NOT NULL
             FROM api_tokens t JOIN users u ON u.id = t.user_id
             WHERE ? IS NULL OR t.user_id = ? ORDER BY t.id",
        )?;
        Ok(stmt
            .query_map(params![user_id, user_id], token_row)?
            .collect::<duckdb::Result<_>>()?)
    }

    fn token_info(&self, id: i64) -> Result<TokenInfo, StoreError> {
        self.connection
            .query_row(
                "SELECT t.id, t.user_id, u.username, t.name, t.scope, t.created_at::VARCHAR,
                        t.revoked_at IS NOT NULL
                 FROM api_tokens t JOIN users u ON u.id = t.user_id WHERE t.id = ?",
                [id],
                token_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::Missing("The token does not exist.".into()))
    }

    /// The enabled user and scope of a live API token.
    pub fn token(&self, token_hash: &str) -> Result<Option<(UserInfo, Scope)>, StoreError> {
        let found: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT t.user_id, t.scope FROM api_tokens t JOIN users u ON u.id = t.user_id
                 WHERE t.token_hash = ? AND t.revoked_at IS NULL AND u.enabled",
                [token_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match found {
            Some((user_id, scope)) => Ok(Some((
                self.user(user_id)?,
                Scope::from_key(&scope).ok_or(StoreError::Unavailable)?,
            ))),
            None => Ok(None),
        }
    }

    /// Revokes a token. With `owner`, only that user's token can be revoked.
    pub fn revoke_token(&mut self, id: i64, owner: Option<i64>) -> Result<TokenInfo, StoreError> {
        let token = self.token_info(id)?;
        if owner.is_some_and(|owner| owner != token.user_id) {
            return Err(StoreError::Missing("The token does not exist.".into()));
        }
        self.connection.execute(
            "UPDATE api_tokens SET revoked_at = CURRENT_TIMESTAMP WHERE id = ? AND revoked_at IS NULL",
            [id],
        )?;
        self.token_info(id)
    }
}

/// Seconds since 1970, so expiry needs no time-zone support in the database.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn dedup(mut roles: Vec<Role>) -> Vec<Role> {
    roles.sort();
    roles.dedup();
    roles
}

fn user_row(row: &Row<'_>) -> duckdb::Result<UserInfo> {
    let roles: String = row.get(5)?;
    Ok(UserInfo {
        id: row.get(0)?,
        username: row.get(1)?,
        full_name: row.get(2)?,
        enabled: row.get(3)?,
        created_at: row.get(4)?,
        roles: roles.split(',').filter_map(Role::from_key).collect(),
    })
}

fn token_row(row: &Row<'_>) -> duckdb::Result<TokenInfo> {
    let scope: String = row.get(4)?;
    Ok(TokenInfo {
        id: row.get(0)?,
        user_id: row.get(1)?,
        username: row.get(2)?,
        name: row.get(3)?,
        scope: Scope::from_key(&scope).unwrap_or(Scope::Read),
        created_at: row.get(5)?,
        revoked: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn admin(store: &mut Store, name: &str) -> UserInfo {
        store
            .create_user(NewUserRecord {
                username: name.into(),
                full_name: "Asha".into(),
                password_hash: "hash".into(),
                roles: vec![Role::SystemManager, Role::SystemManager],
            })
            .unwrap()
    }

    #[test]
    fn users_have_unique_names_and_roles() {
        let mut store = book();
        assert_eq!(store.user_count().unwrap(), 0);
        let asha = admin(&mut store, "asha");
        assert_eq!(asha.roles, vec![Role::SystemManager]);
        assert!(matches!(
            store.create_user(NewUserRecord {
                username: "asha".into(),
                full_name: "Other".into(),
                password_hash: "x".into(),
                roles: vec![Role::Viewer],
            }),
            Err(StoreError::Conflict(_))
        ));
        let (found, hash) = store.credentials("asha").unwrap().unwrap();
        assert_eq!((found.id, hash.as_str()), (asha.id, "hash"));
        assert!(store.credentials("nobody").unwrap().is_none());
    }

    #[test]
    fn last_system_manager_is_kept() {
        let mut store = book();
        let asha = admin(&mut store, "asha");
        let demote = UserUpdate {
            roles: Some(vec![Role::Viewer]),
            ..Default::default()
        };
        assert!(matches!(
            store.update_user(asha.id, demote),
            Err(StoreError::Conflict(_))
        ));
        let ravi = admin(&mut store, "ravi");
        let disabled = store
            .update_user(
                ravi.id,
                UserUpdate {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!disabled.enabled);
        // Ravi is disabled, so Asha is still the last enabled System Manager.
        assert!(matches!(
            store.update_user(
                asha.id,
                UserUpdate {
                    enabled: Some(false),
                    ..Default::default()
                }
            ),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn sessions_expire_and_end_on_password_change() {
        let mut store = book();
        let asha = admin(&mut store, "asha");
        store.create_session("live", asha.id, "csrf", 3600).unwrap();
        store.create_session("old", asha.id, "csrf", -1).unwrap();
        let live = store.session("live").unwrap().unwrap();
        assert_eq!((live.user.id, live.csrf_token.as_str()), (asha.id, "csrf"));
        assert!(store.session("old").unwrap().is_none());
        assert_eq!(store.purge_expired_sessions().unwrap(), 1);

        store
            .update_user(
                asha.id,
                UserUpdate {
                    password_hash: Some("new".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(store.session("live").unwrap().is_none());
        store.create_session("again", asha.id, "c2", 3600).unwrap();
        store.delete_session("again").unwrap();
        assert!(store.session("again").unwrap().is_none());
    }

    #[test]
    fn tokens_carry_a_scope_and_can_be_revoked_by_their_owner() {
        let mut store = book();
        let asha = admin(&mut store, "asha");
        let ravi = admin(&mut store, "ravi");
        let token = store
            .create_token(asha.id, "Claude", "h1", Scope::Draft)
            .unwrap();
        assert_eq!(token.scope, Scope::Draft);
        let (user, scope) = store.token("h1").unwrap().unwrap();
        assert_eq!((user.id, scope), (asha.id, Scope::Draft));
        assert!(store.create_token(asha.id, " ", "h2", Scope::Read).is_err());
        assert!(matches!(
            store.revoke_token(token.id, Some(ravi.id)),
            Err(StoreError::Missing(_))
        ));
        assert!(store.revoke_token(token.id, Some(asha.id)).unwrap().revoked);
        assert!(store.token("h1").unwrap().is_none());
        assert_eq!(store.tokens(Some(asha.id)).unwrap().len(), 1);
        assert!(store.tokens(Some(ravi.id)).unwrap().is_empty());
    }

    #[test]
    fn disabled_users_lose_tokens_and_sessions() {
        let mut store = book();
        admin(&mut store, "asha");
        let ravi = admin(&mut store, "ravi");
        store
            .create_token(ravi.id, "CLI", "t", Scope::Read)
            .unwrap();
        store.create_session("s", ravi.id, "c", 3600).unwrap();
        store
            .update_user(
                ravi.id,
                UserUpdate {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(store.token("t").unwrap().is_none());
        assert!(store.session("s").unwrap().is_none());
    }
}
