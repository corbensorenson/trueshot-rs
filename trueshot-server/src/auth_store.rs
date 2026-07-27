use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRefreshSession {
    pub subject: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub session_version: u64,
    pub expires_at: i64,
    pub issued_at: i64,
    pub last_seen: i64,
    pub csrf_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPairingCode {
    pub scopes: Vec<String>,
    pub label: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub password_hash: String,
    pub created_at: i64,
    pub last_login: Option<i64>,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredLoginThrottle {
    pub failed_attempts: i64,
    pub window_started: i64,
    pub locked_until: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredApiToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used: Option<i64>,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredShareLink {
    pub token_hash: String,
    pub project_id: String,
    pub asset_path: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub max_uses: Option<i64>,
    pub uses: i64,
    pub allow_download: bool,
    pub allow_embed: bool,
    pub revoked: bool,
    pub last_access: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareReferrerCount {
    pub referrer: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareAnalyticsSummary {
    pub views: i64,
    pub asset_requests: i64,
    pub downloads: i64,
    pub embeds: i64,
    pub last_access: Option<i64>,
    pub top_referrers: Vec<ShareReferrerCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSharePublic {
    pub token_hash: String,
    pub public_alias_hash: String,
    pub short_code: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicShareRecord {
    pub token_hash: String,
    pub short_code: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub project_id: String,
    pub asset_path: String,
    pub expires_at: i64,
    pub allow_embed: bool,
    pub allow_download: bool,
    pub last_access: Option<i64>,
    pub views: i64,
}

#[derive(Debug, Clone)]
pub struct AuthStore {
    pool: SqlitePool,
}

impl AuthStore {
    pub async fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create auth db dir: {}", parent.display()))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("Failed to open auth db: {}", path.display()))?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    #[cfg(test)]
    pub async fn close_for_test(&self) {
        self.pool.close().await;
    }

    #[cfg(test)]
    pub async fn set_user_active_for_test(&self, user_id: &str, active: bool) -> Result<()> {
        sqlx::query("UPDATE users SET active = ? WHERE id = ?;")
            .bind(if active { 1 } else { 0 })
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub async fn public_token_storage_for_test(
        &self,
        token_hash: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let row = sqlx::query(
            "SELECT public_token, public_alias_hash FROM share_public WHERE token_hash = ?;",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok((
                row.try_get("public_token")?,
                row.try_get("public_alias_hash")?,
            ))
        })
        .transpose()
    }

    #[cfg(test)]
    pub async fn restore_legacy_public_token_for_test(
        &self,
        token_hash: &str,
        token: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE share_public SET public_token = ?, public_alias_hash = NULL WHERE token_hash = ?;",
        )
        .bind(token)
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS refresh_sessions (
                refresh_hash TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                role TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                issued_at INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                csrf_token TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS refresh_sessions_subject_idx
            ON refresh_sessions(subject);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS refresh_sessions_expires_idx
            ON refresh_sessions(expires_at);
            "#,
        )
        .execute(&self.pool)
        .await?;
        let refresh_columns = sqlx::query("PRAGMA table_info(refresh_sessions);")
            .fetch_all(&self.pool)
            .await?;
        if !refresh_columns.iter().any(|row| {
            row.try_get::<String, _>("name")
                .is_ok_and(|name| name == "session_version")
        }) {
            sqlx::query(
                "ALTER TABLE refresh_sessions ADD COLUMN session_version INTEGER NOT NULL DEFAULT 0;",
            )
            .execute(&self.pool)
            .await?;
        }
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS access_subject_versions (
                subject TEXT PRIMARY KEY,
                version INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS revoked_access_tokens (
                jti TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL,
                revoked_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS revoked_access_tokens_expires_idx
            ON revoked_access_tokens(expires_at);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pairing_codes (
                code TEXT PRIMARY KEY,
                scopes_json TEXT NOT NULL,
                label TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS pairing_codes_expires_idx
            ON pairing_codes(expires_at);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS auth_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                role TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_login INTEGER,
                active INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS users_role_idx
            ON users(role);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS login_failures (
                identity_hash TEXT PRIMARY KEY,
                failed_attempts INTEGER NOT NULL,
                window_started INTEGER NOT NULL,
                locked_until INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS login_failures_updated_idx
            ON login_failures(updated_at);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_tokens (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                scopes_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                last_used INTEGER,
                revoked INTEGER NOT NULL,
                FOREIGN KEY(user_id) REFERENCES users(id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS api_tokens_user_idx
            ON api_tokens(user_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS share_links (
                token_hash TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                asset_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                max_uses INTEGER,
                uses INTEGER NOT NULL,
                allow_download INTEGER NOT NULL,
                allow_embed INTEGER NOT NULL,
                revoked INTEGER NOT NULL,
                last_access INTEGER
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS share_public (
                token_hash TEXT PRIMARY KEY,
                public_token TEXT NOT NULL,
                short_code TEXT NOT NULL UNIQUE,
                title TEXT,
                description TEXT,
                tags_json TEXT,
                cover_path TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_public INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        let public_columns = sqlx::query("PRAGMA table_info(share_public);")
            .fetch_all(&self.pool)
            .await?;
        if !public_columns.iter().any(|row| {
            row.try_get::<String, _>("name")
                .is_ok_and(|name| name == "public_alias_hash")
        }) {
            sqlx::query("ALTER TABLE share_public ADD COLUMN public_alias_hash TEXT;")
                .execute(&self.pool)
                .await?;
        }
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS share_public_alias_hash_idx
            ON share_public(public_alias_hash)
            WHERE public_alias_hash IS NOT NULL AND public_alias_hash != '';
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS share_access (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_hash TEXT NOT NULL,
                event TEXT NOT NULL,
                accessed_at INTEGER NOT NULL,
                ip TEXT,
                user_agent TEXT,
                referrer TEXT,
                embed INTEGER NOT NULL,
                download INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_public_public_idx
            ON share_public(is_public);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_public_updated_idx
            ON share_public(updated_at);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_links_expires_idx
            ON share_links(expires_at);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_links_project_idx
            ON share_links(project_id);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_access_token_idx
            ON share_access(token_hash);
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS share_access_time_idx
            ON share_access(accessed_at);
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_refresh_session(
        &self,
        refresh_hash: &str,
        session: &StoredRefreshSession,
    ) -> Result<()> {
        let scopes_json = serde_json::to_string(&session.scopes)?;
        sqlx::query(
            r#"
            INSERT INTO refresh_sessions
            (refresh_hash, subject, role, scopes_json, session_version, expires_at, issued_at, last_seen, csrf_token)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(refresh_hash)
        .bind(&session.subject)
        .bind(&session.role)
        .bind(scopes_json)
        .bind(i64::try_from(session.session_version).context("Session version exceeds SQLite range")?)
        .bind(session.expires_at)
        .bind(session.issued_at)
        .bind(session.last_seen)
        .bind(&session.csrf_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_refresh_session(
        &self,
        refresh_hash: &str,
    ) -> Result<Option<StoredRefreshSession>> {
        let row = sqlx::query(
            r#"
            SELECT subject, role, scopes_json, session_version, expires_at, issued_at, last_seen, csrf_token
            FROM refresh_sessions
            WHERE refresh_hash = ?;
            "#,
        )
        .bind(refresh_hash)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let scopes_json: String = row.try_get("scopes_json")?;
        let scopes: Vec<String> = serde_json::from_str(&scopes_json)?;
        Ok(Some(StoredRefreshSession {
            subject: row.try_get("subject")?,
            role: row.try_get("role")?,
            scopes,
            session_version: u64::try_from(row.try_get::<i64, _>("session_version")?)
                .context("Stored session version is negative")?,
            expires_at: row.try_get("expires_at")?,
            issued_at: row.try_get("issued_at")?,
            last_seen: row.try_get("last_seen")?,
            csrf_token: row.try_get("csrf_token")?,
        }))
    }

    pub async fn delete_refresh_session(&self, refresh_hash: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM refresh_sessions WHERE refresh_hash = ?;")
            .bind(refresh_hash)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_refresh_sessions_for_subject(&self, subject: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM refresh_sessions WHERE subject = ?;")
            .bind(subject)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn prune_refresh_sessions(&self, now: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM refresh_sessions WHERE expires_at <= ?;")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn access_subject_version(&self, subject: &str) -> Result<u64> {
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM access_subject_versions WHERE subject = ?;",
        )
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(0);
        u64::try_from(version).context("Stored access-token subject version is negative")
    }

    pub async fn increment_access_subject_version(&self, subject: &str, now: i64) -> Result<u64> {
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO access_subject_versions (subject, version, updated_at)
            VALUES (?, 1, ?)
            ON CONFLICT(subject) DO UPDATE SET
                version = CASE
                    WHEN access_subject_versions.version < 9223372036854775807
                        THEN access_subject_versions.version + 1
                    ELSE access_subject_versions.version
                END,
                updated_at = excluded.updated_at
            RETURNING version;
            "#,
        )
        .bind(subject)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        if version == i64::MAX {
            anyhow::bail!("Access-token subject version exhausted");
        }
        u64::try_from(version).context("Stored access-token subject version is negative")
    }

    pub async fn revoke_access_token(&self, jti: &str, expires_at: i64, now: i64) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO revoked_access_tokens (jti, expires_at, revoked_at)
            VALUES (?, ?, ?)
            ON CONFLICT(jti) DO UPDATE SET
                expires_at = MAX(revoked_access_tokens.expires_at, excluded.expires_at),
                revoked_at = MIN(revoked_access_tokens.revoked_at, excluded.revoked_at);
            "#,
        )
        .bind(jti)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn access_token_is_revoked(&self, jti: &str, now: i64) -> Result<bool> {
        let revoked = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(1) FROM revoked_access_tokens WHERE jti = ? AND expires_at > ?;",
        )
        .bind(jti)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(revoked != 0)
    }

    pub async fn prune_revoked_access_tokens(&self, now: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM revoked_access_tokens WHERE expires_at <= ?;")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn insert_pairing_code(
        &self,
        code: &str,
        scopes: &[String],
        label: Option<&String>,
        created_at: i64,
        expires_at: i64,
    ) -> Result<bool> {
        let scopes_json = serde_json::to_string(scopes)?;
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO pairing_codes
            (code, scopes_json, label, created_at, expires_at)
            VALUES (?, ?, ?, ?, ?);
            "#,
        )
        .bind(code)
        .bind(scopes_json)
        .bind(label)
        .bind(created_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn consume_pairing_code(
        &self,
        code: &str,
        now: i64,
    ) -> Result<Option<StoredPairingCode>> {
        let row = sqlx::query(
            r#"
            SELECT scopes_json, label, created_at, expires_at
            FROM pairing_codes
            WHERE code = ?;
            "#,
        )
        .bind(code)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        let expires_at: i64 = row.try_get("expires_at")?;
        if expires_at <= now {
            let _ = self.delete_pairing_code(code).await?;
            return Ok(None);
        }

        let deleted = sqlx::query("DELETE FROM pairing_codes WHERE code = ? AND expires_at > ?;")
            .bind(code)
            .bind(now)
            .execute(&self.pool)
            .await?;
        if deleted.rows_affected() != 1 {
            return Ok(None);
        }

        let scopes_json: String = row.try_get("scopes_json")?;
        let scopes: Vec<String> = serde_json::from_str(&scopes_json)?;
        Ok(Some(StoredPairingCode {
            scopes,
            label: row.try_get("label")?,
            created_at: row.try_get("created_at")?,
            expires_at,
        }))
    }

    pub async fn delete_pairing_code(&self, code: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM pairing_codes WHERE code = ?;")
            .bind(code)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn prune_pairing_codes(&self, now: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM pairing_codes WHERE expires_at <= ?;")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM auth_settings WHERE key = ?;")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|row| row.try_get("value").ok()))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO auth_settings (key, value)
            VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value;
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn any_admin_exists(&self) -> Result<bool> {
        let row =
            sqlx::query("SELECT COUNT(1) as cnt FROM users WHERE role = 'Admin' AND active = 1;")
                .fetch_one(&self.pool)
                .await?;
        let count: i64 = row.try_get("cnt")?;
        Ok(count > 0)
    }

    pub async fn create_user(&self, user: &StoredUser) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, email, name, role, password_hash, created_at, last_login, active)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.role)
        .bind(&user.password_hash)
        .bind(user.created_at)
        .bind(user.last_login)
        .bind(if user.active { 1 } else { 0 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<StoredUser>> {
        let row = sqlx::query(
            r#"
            SELECT id, email, name, role, password_hash, created_at, last_login, active
            FROM users WHERE email = ? COLLATE NOCASE;
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(StoredUser {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            name: row.try_get("name")?,
            role: row.try_get("role")?,
            password_hash: row.try_get("password_hash")?,
            created_at: row.try_get("created_at")?,
            last_login: row.try_get("last_login")?,
            active: row.try_get::<i64, _>("active")? == 1,
        }))
    }

    pub async fn get_user_by_id(&self, user_id: &str) -> Result<Option<StoredUser>> {
        let row = sqlx::query(
            r#"
            SELECT id, email, name, role, password_hash, created_at, last_login, active
            FROM users WHERE id = ?;
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(StoredUser {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            name: row.try_get("name")?,
            role: row.try_get("role")?,
            password_hash: row.try_get("password_hash")?,
            created_at: row.try_get("created_at")?,
            last_login: row.try_get("last_login")?,
            active: row.try_get::<i64, _>("active")? == 1,
        }))
    }

    pub async fn update_user_last_login(&self, user_id: &str, now: i64) -> Result<()> {
        sqlx::query("UPDATE users SET last_login = ? WHERE id = ?;")
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_login_throttle(
        &self,
        identity_hash: &str,
    ) -> Result<Option<StoredLoginThrottle>> {
        let row = sqlx::query(
            r#"
            SELECT failed_attempts, window_started, locked_until, updated_at
            FROM login_failures
            WHERE identity_hash = ?;
            "#,
        )
        .bind(identity_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StoredLoginThrottle {
                failed_attempts: row.try_get("failed_attempts")?,
                window_started: row.try_get("window_started")?,
                locked_until: row.try_get("locked_until")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .transpose()
    }

    pub async fn record_login_failure(
        &self,
        identity_hash: &str,
        now: i64,
        window_seconds: i64,
        threshold: i64,
        base_lock_seconds: i64,
        max_lock_seconds: i64,
    ) -> Result<StoredLoginThrottle> {
        let row = sqlx::query(
            r#"
            INSERT INTO login_failures
                (identity_hash, failed_attempts, window_started, locked_until, updated_at)
            VALUES (?, 1, ?, 0, ?)
            ON CONFLICT(identity_hash) DO UPDATE SET
                locked_until = CASE
                    WHEN excluded.updated_at - login_failures.window_started >= ?
                        THEN 0
                    WHEN login_failures.failed_attempts + 1 >= ?
                        THEN MAX(
                            login_failures.locked_until,
                            excluded.updated_at + MIN(
                                ?,
                                ? * (1 << MIN(
                                    10,
                                    login_failures.failed_attempts + 1 - ?
                                ))
                            )
                        )
                    ELSE login_failures.locked_until
                END,
                failed_attempts = CASE
                    WHEN excluded.updated_at - login_failures.window_started >= ?
                        THEN 1
                    ELSE MIN(login_failures.failed_attempts + 1, 1000000)
                END,
                window_started = CASE
                    WHEN excluded.updated_at - login_failures.window_started >= ?
                        THEN excluded.updated_at
                    ELSE login_failures.window_started
                END,
                updated_at = excluded.updated_at
            RETURNING failed_attempts, window_started, locked_until, updated_at;
            "#,
        )
        .bind(identity_hash)
        .bind(now)
        .bind(now)
        .bind(window_seconds)
        .bind(threshold)
        .bind(max_lock_seconds)
        .bind(base_lock_seconds)
        .bind(threshold)
        .bind(window_seconds)
        .bind(window_seconds)
        .fetch_one(&self.pool)
        .await?;
        Ok(StoredLoginThrottle {
            failed_attempts: row.try_get("failed_attempts")?,
            window_started: row.try_get("window_started")?,
            locked_until: row.try_get("locked_until")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    pub async fn clear_login_failures(&self, identity_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM login_failures WHERE identity_hash = ?;")
            .bind(identity_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn prune_login_failures(&self, cutoff: i64) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM login_failures WHERE updated_at < ? AND locked_until < ?;")
                .bind(cutoff)
                .bind(cutoff)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    #[cfg(test)]
    pub async fn set_login_lock_for_test(
        &self,
        identity_hash: &str,
        locked_until: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE login_failures SET locked_until = ? WHERE identity_hash = ?;")
            .bind(locked_until)
            .bind(identity_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_api_token(&self, token: &StoredApiToken, token_hash: &str) -> Result<()> {
        let scopes_json = serde_json::to_string(&token.scopes)?;
        sqlx::query(
            r#"
            INSERT INTO api_tokens
            (id, user_id, name, token_hash, scopes_json, created_at, expires_at, last_used, revoked)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&token.id)
        .bind(&token.user_id)
        .bind(&token.name)
        .bind(token_hash)
        .bind(scopes_json)
        .bind(token.created_at)
        .bind(token.expires_at)
        .bind(token.last_used)
        .bind(if token.revoked { 1 } else { 0 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_api_tokens(&self, user_id: &str) -> Result<Vec<StoredApiToken>> {
        let rows = sqlx::query(
            r#"
            SELECT id, user_id, name, scopes_json, created_at, expires_at, last_used, revoked
            FROM api_tokens
            WHERE user_id = ?
            ORDER BY created_at DESC;
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut tokens = Vec::new();
        for row in rows {
            let scopes_json: String = row.try_get("scopes_json")?;
            let scopes: Vec<String> = serde_json::from_str(&scopes_json)?;
            tokens.push(StoredApiToken {
                id: row.try_get("id")?,
                user_id: row.try_get("user_id")?,
                name: row.try_get("name")?,
                scopes,
                created_at: row.try_get("created_at")?,
                expires_at: row.try_get("expires_at")?,
                last_used: row.try_get("last_used")?,
                revoked: row.try_get::<i64, _>("revoked")? == 1,
            });
        }
        Ok(tokens)
    }

    pub async fn revoke_api_token(&self, token_id: &str) -> Result<u64> {
        let result = sqlx::query("UPDATE api_tokens SET revoked = 1 WHERE id = ?;")
            .bind(token_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn get_api_token_by_hash(&self, token_hash: &str) -> Result<Option<StoredApiToken>> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, name, scopes_json, created_at, expires_at, last_used, revoked
            FROM api_tokens
            WHERE token_hash = ?;
            "#,
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let scopes_json: String = row.try_get("scopes_json")?;
        let scopes: Vec<String> = serde_json::from_str(&scopes_json)?;
        Ok(Some(StoredApiToken {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            scopes,
            created_at: row.try_get("created_at")?,
            expires_at: row.try_get("expires_at")?,
            last_used: row.try_get("last_used")?,
            revoked: row.try_get::<i64, _>("revoked")? == 1,
        }))
    }

    pub async fn touch_api_token(&self, token_id: &str, now: i64) -> Result<()> {
        sqlx::query("UPDATE api_tokens SET last_used = ? WHERE id = ?;")
            .bind(now)
            .bind(token_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_share_link(&self, link: &StoredShareLink) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO share_links (
                token_hash,
                project_id,
                asset_path,
                created_at,
                expires_at,
                max_uses,
                uses,
                allow_download,
                allow_embed,
                revoked,
                last_access
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&link.token_hash)
        .bind(&link.project_id)
        .bind(&link.asset_path)
        .bind(link.created_at)
        .bind(link.expires_at)
        .bind(link.max_uses)
        .bind(link.uses)
        .bind(if link.allow_download { 1 } else { 0 })
        .bind(if link.allow_embed { 1 } else { 0 })
        .bind(if link.revoked { 1 } else { 0 })
        .bind(link.last_access)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_share_link(&self, token_hash: &str) -> Result<Option<StoredShareLink>> {
        let row = sqlx::query(
            r#"
            SELECT token_hash, project_id, asset_path, created_at, expires_at, max_uses, uses,
                   allow_download, allow_embed, revoked, last_access
            FROM share_links
            WHERE token_hash = ?;
            "#,
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_share_link))
    }

    pub async fn consume_share_link(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<StoredShareLink>> {
        let res = sqlx::query(
            r#"
            UPDATE share_links
            SET uses = uses + 1, last_access = ?
            WHERE token_hash = ?
              AND revoked = 0
              AND expires_at > ?
              AND (max_uses IS NULL OR uses < max_uses);
            "#,
        )
        .bind(now)
        .bind(token_hash)
        .bind(now)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_share_link(token_hash).await
    }

    pub async fn record_share_access(
        &self,
        token_hash: &str,
        event: &str,
        accessed_at: i64,
        ip: Option<&str>,
        user_agent: Option<&str>,
        referrer: Option<&str>,
        embed: bool,
        download: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO share_access
                (token_hash, event, accessed_at, ip, user_agent, referrer, embed, download)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(token_hash)
        .bind(event)
        .bind(accessed_at)
        .bind(ip)
        .bind(user_agent)
        .bind(referrer)
        .bind(if embed { 1 } else { 0 })
        .bind(if download { 1 } else { 0 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn share_analytics(&self, token_hash: &str) -> Result<ShareAnalyticsSummary> {
        let views: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM share_access
            WHERE token_hash = ? AND event = 'meta';
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        let asset_requests: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM share_access
            WHERE token_hash = ? AND event = 'asset';
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        let downloads: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM share_access
            WHERE token_hash = ? AND download = 1;
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        let embeds: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM share_access
            WHERE token_hash = ? AND embed = 1;
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        let last_access: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT MAX(accessed_at) FROM share_access
            WHERE token_hash = ?;
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            r#"
            SELECT referrer, COUNT(*) as count
            FROM share_access
            WHERE token_hash = ?
              AND referrer IS NOT NULL
              AND referrer != ''
            GROUP BY referrer
            ORDER BY count DESC
            LIMIT 5;
            "#,
        )
        .bind(token_hash)
        .fetch_all(&self.pool)
        .await?;
        let mut top_referrers = Vec::new();
        for row in rows {
            let referrer: String = row.try_get("referrer")?;
            let count: i64 = row.try_get("count")?;
            top_referrers.push(ShareReferrerCount { referrer, count });
        }
        Ok(ShareAnalyticsSummary {
            views,
            asset_requests,
            downloads,
            embeds,
            last_access,
            top_referrers,
        })
    }

    pub async fn upsert_share_public(&self, entry: &StoredSharePublic) -> Result<()> {
        let tags_json = serde_json::to_string(&entry.tags)?;
        sqlx::query(
            r#"
            INSERT INTO share_public (
                token_hash,
                public_token,
                public_alias_hash,
                short_code,
                title,
                description,
                tags_json,
                cover_path,
                created_at,
                updated_at,
                is_public
            )
            VALUES (?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(token_hash) DO UPDATE SET
                public_token = '',
                public_alias_hash = excluded.public_alias_hash,
                short_code = excluded.short_code,
                title = excluded.title,
                description = excluded.description,
                tags_json = excluded.tags_json,
                cover_path = excluded.cover_path,
                updated_at = excluded.updated_at,
                is_public = excluded.is_public;
            "#,
        )
        .bind(&entry.token_hash)
        .bind(&entry.public_alias_hash)
        .bind(&entry.short_code)
        .bind(&entry.title)
        .bind(&entry.description)
        .bind(tags_json)
        .bind(&entry.cover_path)
        .bind(entry.created_at)
        .bind(entry.updated_at)
        .bind(if entry.is_public { 1 } else { 0 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn public_share_alias_states(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT token_hash, public_token, public_alias_hash FROM share_public ORDER BY token_hash;",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("token_hash")?,
                    row.try_get("public_token")?,
                    row.try_get("public_alias_hash")?,
                ))
            })
            .collect()
    }

    pub async fn begin_public_token_scrub(&self) -> Result<()> {
        sqlx::query("PRAGMA secure_delete = ON;")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn finish_public_token_scrub(&self) -> Result<()> {
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE);")
            .execute(&self.pool)
            .await;
        sqlx::query("VACUUM;").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_public_alias_hash(
        &self,
        token_hash: &str,
        public_alias_hash: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE share_public
            SET public_alias_hash = ?, public_token = ''
            WHERE token_hash = ?;
            "#,
        )
        .bind(public_alias_hash)
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn resolve_public_alias_hash(
        &self,
        public_alias_hash: &str,
    ) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT token_hash FROM share_public WHERE public_alias_hash = ? LIMIT 1;",
        )
        .bind(public_alias_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn get_share_public(&self, token_hash: &str) -> Result<Option<StoredSharePublic>> {
        let row = sqlx::query(
            r#"
            SELECT token_hash, COALESCE(public_alias_hash, '') AS public_alias_hash, short_code, title, description, tags_json, cover_path, created_at, updated_at, is_public
            FROM share_public
            WHERE token_hash = ?;
            "#,
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let tags_json: String = row.try_get("tags_json")?;
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        Ok(Some(StoredSharePublic {
            token_hash: row.try_get("token_hash")?,
            public_alias_hash: row.try_get("public_alias_hash")?,
            short_code: row.try_get("short_code")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            tags,
            cover_path: row.try_get("cover_path")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            is_public: row.try_get::<i64, _>("is_public")? != 0,
        }))
    }

    pub async fn get_share_public_by_code(
        &self,
        short_code: &str,
    ) -> Result<Option<StoredSharePublic>> {
        let row = sqlx::query(
            r#"
            SELECT token_hash, COALESCE(public_alias_hash, '') AS public_alias_hash, short_code, title, description, tags_json, cover_path, created_at, updated_at, is_public
            FROM share_public
            WHERE short_code = ?;
            "#,
        )
        .bind(short_code)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let tags_json: String = row.try_get("tags_json")?;
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        Ok(Some(StoredSharePublic {
            token_hash: row.try_get("token_hash")?,
            public_alias_hash: row.try_get("public_alias_hash")?,
            short_code: row.try_get("short_code")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            tags,
            cover_path: row.try_get("cover_path")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            is_public: row.try_get::<i64, _>("is_public")? != 0,
        }))
    }

    pub async fn list_public_shares(
        &self,
        now: i64,
        limit: i64,
        offset: i64,
        tag: Option<&str>,
        sort: Option<&str>,
    ) -> Result<Vec<PublicShareRecord>> {
        let mut query = String::from(
            r#"
            SELECT
                sp.token_hash,
                sp.short_code,
                sp.title,
                sp.description,
                sp.tags_json,
                sp.cover_path,
                sp.created_at,
                sp.updated_at,
                sl.project_id,
                sl.asset_path,
                sl.expires_at,
                sl.allow_embed,
                sl.allow_download,
                sl.last_access,
                (
                    SELECT COUNT(*) FROM share_access sa
                    WHERE sa.token_hash = sp.token_hash AND sa.event = 'asset'
                ) AS views
            FROM share_public sp
            JOIN share_links sl ON sl.token_hash = sp.token_hash
            WHERE sp.is_public = 1
              AND sl.revoked = 0
              AND sl.expires_at > ?
              AND sl.allow_embed = 1
            "#,
        );

        if tag.is_some() {
            query.push_str(" AND sp.tags_json LIKE ? ");
        }

        match sort {
            Some("popular") => query.push_str(" ORDER BY views DESC, sp.updated_at DESC "),
            _ => query.push_str(" ORDER BY sp.updated_at DESC "),
        }
        query.push_str(" LIMIT ? OFFSET ? ");

        let mut q = sqlx::query(&query).bind(now);
        if let Some(tag) = tag {
            q = q.bind(format!("%{}%", tag));
        }
        q = q.bind(limit).bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        let mut items = Vec::new();
        for row in rows {
            let tags_json: String = row.try_get("tags_json")?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            items.push(PublicShareRecord {
                token_hash: row.try_get("token_hash")?,
                short_code: row.try_get("short_code")?,
                title: row.try_get("title")?,
                description: row.try_get("description")?,
                tags,
                cover_path: row.try_get("cover_path")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                project_id: row.try_get("project_id")?,
                asset_path: row.try_get("asset_path")?,
                expires_at: row.try_get("expires_at")?,
                allow_embed: row.try_get::<i64, _>("allow_embed")? != 0,
                allow_download: row.try_get::<i64, _>("allow_download")? != 0,
                last_access: row.try_get("last_access")?,
                views: row.try_get("views")?,
            });
        }
        Ok(items)
    }

    #[allow(dead_code)]
    pub async fn revoke_share_link(&self, token_hash: &str) -> Result<u64> {
        let res = sqlx::query("UPDATE share_links SET revoked = 1 WHERE token_hash = ?;")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    #[allow(dead_code)]
    pub async fn prune_share_links(&self, now: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM share_links WHERE expires_at <= ? OR revoked = 1;")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_share_link(row: sqlx::sqlite::SqliteRow) -> StoredShareLink {
    StoredShareLink {
        token_hash: row.get("token_hash"),
        project_id: row.get("project_id"),
        asset_path: row.get("asset_path"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        max_uses: row.get("max_uses"),
        uses: row.get("uses"),
        allow_download: row.get::<i64, _>("allow_download") != 0,
        allow_embed: row.get::<i64, _>("allow_embed") != 0,
        revoked: row.get::<i64, _>("revoked") != 0,
        last_access: row.get("last_access"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migration_adds_refresh_generation_to_existing_database() {
        let directory = tempfile::tempdir().expect("temporary auth directory");
        let path = directory.path().join("auth.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let legacy = SqlitePool::connect_with(options)
            .await
            .expect("legacy auth database");
        sqlx::query(
            r#"
            CREATE TABLE refresh_sessions (
                refresh_hash TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                role TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                issued_at INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                csrf_token TEXT NOT NULL
            );
            "#,
        )
        .execute(&legacy)
        .await
        .expect("legacy refresh schema");
        legacy.close().await;

        let store = AuthStore::new(&path).await.expect("migrated auth store");
        let session = StoredRefreshSession {
            subject: "operator".to_string(),
            role: "Admin".to_string(),
            scopes: vec!["*".to_string()],
            session_version: 7,
            expires_at: 2_000,
            issued_at: 1_000,
            last_seen: 1_000,
            csrf_token: "csrf".to_string(),
        };
        store
            .insert_refresh_session("refresh", &session)
            .await
            .expect("insert versioned refresh");
        assert_eq!(
            store
                .get_refresh_session("refresh")
                .await
                .expect("read refresh")
                .expect("stored refresh")
                .session_version,
            7
        );
    }
}
