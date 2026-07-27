use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::RETRY_AFTER;
use actix_web::{body::EitherBody, Error, HttpMessage, HttpRequest, HttpResponse};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::{
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD},
    Engine as _,
};
use ipnet::IpNet;
use jsonwebtoken::{
    decode, encode, errors::ErrorKind, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::{ready, Ready};
use std::net::IpAddr;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

use crate::auth_store::{
    AuthStore, PublicShareRecord, ShareAnalyticsSummary, StoredApiToken, StoredRefreshSession,
    StoredShareLink, StoredSharePublic, StoredUser,
};
use crate::rate_limit::RateLimiter;
use utoipa::ToSchema;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Guest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: Role,
    pub scopes: Vec<String>,
    pub jti: String,
    pub session_version: u64,
    pub exp: usize,
    pub iat: usize,
    pub iss: String,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub sub: String,
    pub role: Role,
    pub scopes: Vec<String>,
    pub principal: PrincipalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    Session,
    ApiToken,
    BootstrapApiKey,
}

pub const API_TOKEN_SCOPES: &[&str] = &["read", "capture", "process", "export", "license", "admin"];
const HMAC_SECRET_ENV: &str = "TRUESHOT_HMAC_SECRET";
const HMAC_SECRET_FILE_ENV: &str = "TRUESHOT_HMAC_SECRET_FILE";
const HMAC_KEYRING_ENTRY: &str = "server_hmac_secret";
const MIN_HMAC_SECRET_BYTES: usize = 32;
const MAX_HMAC_SECRET_BYTES: usize = 1024;
const LOGIN_FAILURE_WINDOW_SECONDS: i64 = 15 * 60;
const LOGIN_FAILURE_THRESHOLD: i64 = 5;
const LOGIN_BASE_LOCK_SECONDS: i64 = 30;
const LOGIN_MAX_LOCK_SECONDS: i64 = 60 * 60;
const LOGIN_FAILURE_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;
const MAX_LOGIN_EMAIL_BYTES: usize = 320;
const MAX_LOGIN_PASSWORD_BYTES: usize = 1024;

#[allow(dead_code)]
#[derive(Debug)]
pub enum AuthError {
    Missing,
    Invalid,
    Expired,
    NotAuthorized,
    KeychainUnavailable(String),
    RateLimited { retry_after_seconds: u64 },
    Storage(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Missing => write!(f, "Missing auth token"),
            AuthError::Invalid => write!(f, "Invalid auth token"),
            AuthError::Expired => write!(f, "Auth token expired"),
            AuthError::NotAuthorized => write!(f, "Not authorized"),
            AuthError::KeychainUnavailable(msg) => write!(f, "Keychain unavailable: {}", msg),
            AuthError::RateLimited { .. } => write!(f, "Rate limited"),
            AuthError::Storage(msg) => write!(f, "Auth storage error: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

pub struct AuthManager {
    issuer: String,
    secret: Vec<u8>,
    admin_ttl: Duration,
    guest_ttl: Duration,
    refresh_ttl: Duration,
    store: Arc<AuthStore>,
    pairing_rate: Mutex<HashMap<IpAddr, PairingRate>>,
}

impl AuthManager {
    pub fn new_with_secret_path(
        issuer: String,
        admin_ttl: Duration,
        guest_ttl: Duration,
        refresh_ttl: Duration,
        store: Arc<AuthStore>,
        secret_path: Option<&Path>,
        production: bool,
    ) -> Result<Self, AuthError> {
        let secret =
            load_hmac_secret(secret_path, production).map_err(AuthError::KeychainUnavailable)?;
        Ok(Self {
            issuer,
            secret,
            admin_ttl,
            guest_ttl,
            refresh_ttl,
            store,
            pairing_rate: Mutex::new(HashMap::new()),
        })
    }

    pub async fn issue_admin_token(
        &self,
        subject: &str,
        scopes: Vec<String>,
    ) -> Result<String, AuthError> {
        let session_version = self.current_session_version(subject).await?;
        self.issue_token(
            subject,
            Role::Admin,
            scopes,
            self.admin_ttl,
            session_version,
        )
    }

    pub async fn issue_guest_token(
        &self,
        subject: &str,
        scopes: Vec<String>,
    ) -> Result<String, AuthError> {
        let session_version = self.current_session_version(subject).await?;
        self.issue_token(
            subject,
            Role::Guest,
            scopes,
            self.guest_ttl,
            session_version,
        )
    }

    pub fn admin_ttl_seconds(&self) -> u64 {
        self.admin_ttl.as_secs()
    }

    pub fn guest_ttl_seconds(&self) -> u64 {
        self.guest_ttl.as_secs()
    }

    pub fn pairing_ttl_seconds(&self) -> u64 {
        PAIRING_TTL.as_secs()
    }

    pub async fn bootstrap_required(&self) -> Result<bool, AuthError> {
        let complete = self
            .store
            .get_setting("bootstrap_complete")
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .unwrap_or_else(|| "false".to_string());
        if complete == "true" {
            return Ok(false);
        }
        let has_admin = self
            .store
            .any_admin_exists()
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok(!has_admin)
    }

    pub async fn mark_bootstrap_complete(&self) -> Result<(), AuthError> {
        self.store
            .set_setting("bootstrap_complete", "true")
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>, AuthError> {
        self.store
            .get_setting(key)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), AuthError> {
        self.store
            .set_setting(key, value)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn create_admin_user(
        &self,
        email: &str,
        name: &str,
        password: &str,
    ) -> Result<StoredUser, AuthError> {
        let has_admin = self
            .store
            .any_admin_exists()
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        if has_admin {
            return Err(AuthError::NotAuthorized);
        }
        let email = email.trim().to_lowercase();
        if email.is_empty() || email.len() > MAX_LOGIN_EMAIL_BYTES {
            return Err(AuthError::Invalid);
        }
        let password_hash = hash_password(password)?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let user = StoredUser {
            id: uuid::Uuid::new_v4().to_string(),
            email,
            name: name.to_string(),
            role: "Admin".to_string(),
            password_hash,
            created_at: now,
            last_login: None,
            active: true,
        };
        self.store
            .create_user(&user)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok(user)
    }

    pub async fn verify_password_login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<StoredUser, AuthError> {
        if email.is_empty()
            || email.len() > MAX_LOGIN_EMAIL_BYTES
            || password.is_empty()
            || password.len() > MAX_LOGIN_PASSWORD_BYTES
        {
            return Err(AuthError::Invalid);
        }
        let identity_hash = login_identity_hash(email);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        if let Some(throttle) = self
            .store
            .get_login_throttle(&identity_hash)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?
        {
            if throttle.locked_until > now {
                return Err(AuthError::RateLimited {
                    retry_after_seconds: (throttle.locked_until - now) as u64,
                });
            }
        }
        let normalized_email = email.trim().to_lowercase();
        let user = self
            .store
            .get_user_by_email(&normalized_email)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        let password_valid = match user.as_ref() {
            Some(user) => verify_password(&user.password_hash, password)?,
            None => verify_password(dummy_password_hash()?, password)?,
        };
        if !password_valid || user.as_ref().is_some_and(|user| !user.active) {
            let throttle = self
                .store
                .record_login_failure(
                    &identity_hash,
                    now,
                    LOGIN_FAILURE_WINDOW_SECONDS,
                    LOGIN_FAILURE_THRESHOLD,
                    LOGIN_BASE_LOCK_SECONDS,
                    LOGIN_MAX_LOCK_SECONDS,
                )
                .await
                .map_err(|error| AuthError::Storage(error.to_string()))?;
            if throttle.locked_until > now {
                return Err(AuthError::RateLimited {
                    retry_after_seconds: (throttle.locked_until - now) as u64,
                });
            }
            return Err(AuthError::Invalid);
        }
        let user = user.ok_or(AuthError::Invalid)?;
        self.store
            .clear_login_failures(&identity_hash)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        self.store
            .update_user_last_login(&user.id, now)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        let _ = self
            .store
            .prune_login_failures(now.saturating_sub(LOGIN_FAILURE_RETENTION_SECONDS))
            .await;
        Ok(user)
    }

    pub async fn create_api_token(
        &self,
        user_id: &str,
        name: &str,
        scopes: Vec<String>,
        expires_at: Option<i64>,
    ) -> Result<(String, StoredApiToken), AuthError> {
        let owner = self
            .store
            .get_user_by_id(user_id)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .ok_or(AuthError::Invalid)?;
        if !owner.active || parse_stored_role(&owner.role)? != Role::Admin {
            return Err(AuthError::NotAuthorized);
        }
        let raw = generate_api_token();
        let hash = hash_api_token(&raw);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let token = StoredApiToken {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            name: name.to_string(),
            scopes,
            created_at: now,
            expires_at,
            last_used: None,
            revoked: false,
        };
        self.store
            .insert_api_token(&token, &hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok((raw, token))
    }

    pub async fn list_api_tokens(&self, user_id: &str) -> Result<Vec<StoredApiToken>, AuthError> {
        self.store
            .list_api_tokens(user_id)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn revoke_api_token(&self, token_id: &str) -> Result<u64, AuthError> {
        self.store
            .revoke_api_token(token_id)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn verify_api_token(&self, raw: &str) -> Result<AuthContext, AuthError> {
        let hash = hash_api_token(raw);
        let token = self
            .store
            .get_api_token_by_hash(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .ok_or(AuthError::Invalid)?;
        if token.revoked {
            return Err(AuthError::Expired);
        }
        if let Some(expires_at) = token.expires_at {
            let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
            if expires_at <= now {
                return Err(AuthError::Expired);
            }
        }
        let owner = self
            .store
            .get_user_by_id(&token.user_id)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .ok_or(AuthError::Invalid)?;
        if !owner.active {
            return Err(AuthError::NotAuthorized);
        }
        let role = parse_stored_role(&owner.role)?;
        let _ = self
            .store
            .touch_api_token(
                &token.id,
                system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?,
            )
            .await;
        Ok(AuthContext {
            sub: token.user_id,
            role,
            scopes: token.scopes,
            principal: PrincipalKind::ApiToken,
        })
    }

    pub async fn create_share_link(
        &self,
        project_id: &str,
        asset_path: &str,
        ttl_seconds: u64,
        max_uses: Option<u64>,
        allow_download: bool,
        allow_embed: bool,
    ) -> Result<(String, StoredShareLink), AuthError> {
        let raw = generate_share_token();
        let hash = hash_share_token(&raw);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let link = StoredShareLink {
            token_hash: hash,
            project_id: project_id.to_string(),
            asset_path: asset_path.to_string(),
            created_at: now,
            expires_at: now + ttl_seconds as i64,
            max_uses: max_uses.map(|v| v as i64),
            uses: 0,
            allow_download,
            allow_embed,
            revoked: false,
            last_access: None,
        };
        self.store
            .insert_share_link(&link)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok((raw, link))
    }

    pub async fn get_share_link(&self, token: &str) -> Result<Option<StoredShareLink>, AuthError> {
        let hash = self.resolve_share_token_hash(token).await?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let link = self
            .store
            .get_share_link(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let Some(link) = link else {
            return Ok(None);
        };
        if link.revoked || link.expires_at <= now {
            return Ok(None);
        }
        if let Some(max_uses) = link.max_uses {
            if link.uses >= max_uses {
                return Ok(None);
            }
        }
        Ok(Some(link))
    }

    pub async fn consume_share_link(
        &self,
        token: &str,
    ) -> Result<Option<StoredShareLink>, AuthError> {
        let hash = self.resolve_share_token_hash(token).await?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        self.store
            .consume_share_link(&hash, now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn record_share_access(
        &self,
        token: &str,
        event: &str,
        accessed_at: i64,
        ip: Option<&str>,
        user_agent: Option<&str>,
        referrer: Option<&str>,
        embed: bool,
        download: bool,
    ) -> Result<(), AuthError> {
        let hash = self.resolve_share_token_hash(token).await?;
        self.store
            .record_share_access(
                &hash,
                event,
                accessed_at,
                ip,
                user_agent,
                referrer,
                embed,
                download,
            )
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn get_share_analytics(
        &self,
        token: &str,
    ) -> Result<ShareAnalyticsSummary, AuthError> {
        let hash = self.resolve_share_token_hash(token).await?;
        self.store
            .share_analytics(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn upsert_share_public(
        &self,
        token: &str,
        public: bool,
        title: Option<String>,
        description: Option<String>,
        tags: Vec<String>,
        cover_path: Option<String>,
        short_code: Option<String>,
    ) -> Result<StoredSharePublic, AuthError> {
        let hash = hash_share_token(token);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let link = self
            .store
            .get_share_link(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let Some(link) = link else {
            return Err(AuthError::Invalid);
        };
        if link.revoked || link.expires_at <= now {
            return Err(AuthError::Expired);
        }

        let normalized_tags = tags
            .into_iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();

        let normalized_code = short_code
            .as_deref()
            .map(normalize_short_code)
            .filter(|code| !code.is_empty());

        let short_code = if let Some(code) = normalized_code {
            if let Some(existing) = self
                .store
                .get_share_public_by_code(&code)
                .await
                .map_err(|e| AuthError::Storage(e.to_string()))?
            {
                if existing.token_hash != hash {
                    return Err(AuthError::Invalid);
                }
            }
            code
        } else {
            let mut code = generate_short_code();
            for _ in 0..5 {
                let existing = self
                    .store
                    .get_share_public_by_code(&code)
                    .await
                    .map_err(|e| AuthError::Storage(e.to_string()))?;
                if existing.is_none() {
                    break;
                }
                code = generate_short_code();
            }
            code
        };

        let existing = self
            .store
            .get_share_public(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let created_at = existing.as_ref().map(|v| v.created_at).unwrap_or(now);

        let entry = StoredSharePublic {
            public_alias_hash: hash_share_token(&self.public_share_alias_for_hash(&hash)),
            token_hash: hash,
            short_code,
            title,
            description,
            tags: normalized_tags,
            cover_path,
            created_at,
            updated_at: now,
            is_public: public,
        };
        self.store
            .upsert_share_public(&entry)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok(entry)
    }

    pub async fn get_share_public(
        &self,
        token: &str,
    ) -> Result<Option<StoredSharePublic>, AuthError> {
        let hash = hash_share_token(token);
        if let Some(entry) = self
            .store
            .get_share_public(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
        {
            return Ok(Some(entry));
        }
        let Some(token_hash) = self
            .store
            .resolve_public_alias_hash(&hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
        else {
            return Ok(None);
        };
        self.store
            .get_share_public(&token_hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn list_public_shares(
        &self,
        limit: i64,
        offset: i64,
        tag: Option<&str>,
        sort: Option<&str>,
    ) -> Result<Vec<PublicShareRecord>, AuthError> {
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        self.store
            .list_public_shares(now, limit, offset, tag, sort)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub async fn get_share_public_by_code(
        &self,
        code: &str,
    ) -> Result<Option<StoredSharePublic>, AuthError> {
        let normalized = normalize_short_code(code);
        self.store
            .get_share_public_by_code(&normalized)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    pub fn public_share_alias_for_hash(&self, token_hash: &str) -> String {
        let mut message = b"trueshot-public-share-alias-v1\0".to_vec();
        message.extend_from_slice(token_hash.as_bytes());
        URL_SAFE_NO_PAD.encode(hmac_sha256(&self.secret, &message))
    }

    pub async fn migrate_public_share_aliases(&self) -> Result<usize, AuthError> {
        let states = self
            .store
            .public_share_alias_states()
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        let changes = states
            .into_iter()
            .filter_map(|(token_hash, legacy_token, stored_alias_hash)| {
                let alias = self.public_share_alias_for_hash(&token_hash);
                let desired_alias_hash = hash_share_token(&alias);
                (!legacy_token.is_empty()
                    || stored_alias_hash.as_deref() != Some(desired_alias_hash.as_str()))
                .then_some((token_hash, desired_alias_hash))
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return Ok(0);
        }
        self.store
            .begin_public_token_scrub()
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        for (token_hash, alias_hash) in &changes {
            self.store
                .set_public_alias_hash(token_hash, alias_hash)
                .await
                .map_err(|error| AuthError::Storage(error.to_string()))?;
        }
        self.store
            .finish_public_token_scrub()
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        Ok(changes.len())
    }

    async fn resolve_share_token_hash(&self, token: &str) -> Result<String, AuthError> {
        let candidate = hash_share_token(token);
        if self
            .store
            .get_share_link(&candidate)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?
            .is_some()
        {
            return Ok(candidate);
        }
        Ok(self
            .store
            .resolve_public_alias_hash(&candidate)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?
            .unwrap_or(candidate))
    }

    pub async fn verify_token(&self, token: &str) -> Result<AuthContext, AuthError> {
        let claims = decode_claims_with_secret(token, &self.issuer, &self.secret)?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        if self
            .store
            .access_token_is_revoked(&claims.jti, now)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?
        {
            return Err(AuthError::Expired);
        }
        let current_version = self.current_session_version(&claims.sub).await?;
        if claims.session_version != current_version {
            return Err(AuthError::Expired);
        }
        let _ = self.store.prune_revoked_access_tokens(now).await;
        Ok(claims_to_context(claims))
    }

    pub async fn issue_session_tokens(
        &self,
        subject: &str,
        role: Role,
        scopes: Vec<String>,
    ) -> Result<SessionTokens, AuthError> {
        let session_version = self.current_session_version(subject).await?;
        self.issue_session_tokens_at_version(subject, role, scopes, session_version)
            .await
    }

    async fn issue_session_tokens_at_version(
        &self,
        subject: &str,
        role: Role,
        scopes: Vec<String>,
        session_version: u64,
    ) -> Result<SessionTokens, AuthError> {
        let access_ttl = match role {
            Role::Admin => self.admin_ttl,
            Role::Guest => self.guest_ttl,
        };
        let access_token =
            self.issue_token(subject, role, scopes.clone(), access_ttl, session_version)?;
        let refresh_token = generate_refresh_token();
        let csrf_token = generate_csrf_token();
        let expires_at = SystemTime::now()
            .checked_add(self.refresh_ttl)
            .ok_or(AuthError::Invalid)?;
        let refresh_hash = hash_refresh_token(&refresh_token);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let expires_at_secs = system_time_to_secs(expires_at).map_err(AuthError::Storage)?;
        self.store
            .prune_refresh_sessions(now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let stored = StoredRefreshSession {
            subject: subject.to_string(),
            role: format!("{:?}", role),
            scopes,
            session_version,
            expires_at: expires_at_secs,
            issued_at: now,
            last_seen: now,
            csrf_token: csrf_token.clone(),
        };
        self.store
            .insert_refresh_session(&refresh_hash, &stored)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok(SessionTokens {
            subject: subject.to_string(),
            role,
            access_token,
            refresh_token,
            csrf_token,
            access_ttl,
            refresh_ttl: self.refresh_ttl,
        })
    }

    pub async fn refresh_session(&self, refresh_token: &str) -> Result<SessionTokens, AuthError> {
        let refresh_hash = hash_refresh_token(refresh_token);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        self.store
            .prune_refresh_sessions(now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let entry = match self
            .store
            .get_refresh_session(&refresh_hash)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
        {
            Some(entry) => entry,
            None => return Err(AuthError::Expired),
        };
        if entry.expires_at <= now {
            let _ = self.store.delete_refresh_session(&refresh_hash).await;
            return Err(AuthError::Expired);
        }
        let current_version = self.current_session_version(&entry.subject).await?;
        if entry.session_version != current_version {
            let _ = self.store.delete_refresh_session(&refresh_hash).await;
            return Err(AuthError::Expired);
        }
        let _ = self.store.delete_refresh_session(&refresh_hash).await;
        let role = match entry.role.as_str() {
            "Admin" => Role::Admin,
            _ => Role::Guest,
        };
        self.issue_session_tokens_at_version(
            &entry.subject,
            role,
            entry.scopes,
            entry.session_version,
        )
        .await
    }

    pub async fn revoke_refresh_token(&self, refresh_token: &str) {
        let refresh_hash = hash_refresh_token(refresh_token);
        let _ = self.store.delete_refresh_session(&refresh_hash).await;
    }

    pub async fn revoke_access_token(&self, token: &str) -> Result<(), AuthError> {
        let claims = decode_claims_with_secret(token, &self.issuer, &self.secret)?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let expires_at = i64::try_from(claims.exp)
            .map_err(|_| AuthError::Storage("Access-token expiry exceeds storage range".into()))?;
        self.store
            .revoke_access_token(&claims.jti, expires_at, now)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))
    }

    pub async fn revoke_all_for_subject(&self, subject: &str) -> Result<(), AuthError> {
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        self.store
            .increment_access_subject_version(subject, now)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        self.store
            .delete_refresh_sessions_for_subject(subject)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        Ok(())
    }

    pub async fn issue_pairing_code(
        &self,
        scopes: Vec<String>,
        label: Option<String>,
    ) -> Result<String, AuthError> {
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let expires_at = now + PAIRING_TTL.as_secs() as i64;
        self.store
            .prune_pairing_codes(now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        for _ in 0..5 {
            let raw = generate_pairing_code();
            let inserted = self
                .store
                .insert_pairing_code(&raw, &scopes, label.as_ref(), now, expires_at)
                .await
                .map_err(|e| AuthError::Storage(e.to_string()))?;
            if inserted {
                return Ok(format_pairing_code(&raw));
            }
        }
        Err(AuthError::Storage(
            "Failed to allocate pairing code".to_string(),
        ))
    }

    pub async fn consume_pairing_code(
        &self,
        code: &str,
        ip: Option<IpAddr>,
    ) -> Result<PairingGrant, AuthError> {
        if let Some(ip) = ip {
            if !self.allow_pairing_attempt(ip) {
                return Err(AuthError::RateLimited {
                    retry_after_seconds: 1,
                });
            }
        }
        let normalized = normalize_pairing_code(code);
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        self.store
            .prune_pairing_codes(now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        let entry = self
            .store
            .consume_pairing_code(&normalized, now)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .ok_or(AuthError::Invalid)?;
        Ok(PairingGrant {
            role: Role::Guest,
            scopes: entry.scopes,
            label: entry.label,
        })
    }

    fn issue_token(
        &self,
        subject: &str,
        role: Role,
        scopes: Vec<String>,
        ttl: Duration,
        session_version: u64,
    ) -> Result<String, AuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::Invalid)?
            .as_secs();
        let exp = now + ttl.as_secs();
        let claims = Claims {
            sub: subject.to_string(),
            role,
            scopes,
            jti: uuid::Uuid::new_v4().to_string(),
            session_version,
            exp: exp as usize,
            iat: now as usize,
            iss: self.issuer.clone(),
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|_| AuthError::Invalid)
    }

    async fn current_session_version(&self, subject: &str) -> Result<u64, AuthError> {
        self.store
            .access_subject_version(subject)
            .await
            .map_err(|error| AuthError::Storage(error.to_string()))
    }

    fn allow_pairing_attempt(&self, ip: IpAddr) -> bool {
        let Ok(mut buckets) = crate::sync_lock::lock(&self.pairing_rate, "auth.pairing_rate")
        else {
            return false;
        };
        let now = Instant::now();
        let bucket = buckets.entry(ip).or_insert(PairingRate {
            tokens: PAIRING_RATE_CAPACITY,
            last_refill: now,
        });
        refill_pairing_bucket(bucket, now);
        if bucket.tokens == 0 {
            return false;
        }
        bucket.tokens -= 1;
        true
    }
}

pub fn require_admin(req: &HttpRequest) -> Result<(), HttpResponse> {
    match req.extensions().get::<AuthContext>() {
        Some(ctx)
            if ctx.role == Role::Admin
                && (ctx.principal != PrincipalKind::ApiToken
                    || api_token_authorized(ctx, req.method().as_str(), req.path())) =>
        {
            Ok(())
        }
        _ => Err(HttpResponse::Forbidden().body("Admin access required")),
    }
}

pub fn require_guest_or_admin(req: &HttpRequest) -> Result<(), HttpResponse> {
    match req.extensions().get::<AuthContext>() {
        Some(_) => Ok(()),
        None => Err(HttpResponse::Unauthorized().body("Unauthorized")),
    }
}

pub fn require_scope(req: &HttpRequest, scope: &str) -> Result<(), HttpResponse> {
    match req.extensions().get::<AuthContext>() {
        Some(ctx) => {
            if ctx.role == Role::Admin && ctx.principal != PrincipalKind::ApiToken {
                return Ok(());
            }
            if scope_is_granted(ctx, scope) {
                Ok(())
            } else {
                Err(HttpResponse::Forbidden().body("Insufficient scope"))
            }
        }
        None => Err(HttpResponse::Unauthorized().body("Unauthorized")),
    }
}

fn parse_stored_role(role: &str) -> Result<Role, AuthError> {
    match role {
        "Admin" | "admin" => Ok(Role::Admin),
        "Guest" | "guest" => Ok(Role::Guest),
        _ => Err(AuthError::Invalid),
    }
}

fn scope_is_granted(ctx: &AuthContext, required: &str) -> bool {
    ctx.scopes
        .iter()
        .any(|scope| scope == "*" || scope == "admin" || scope == required)
        || (required.ends_with(":read") && ctx.scopes.iter().any(|scope| scope == "read"))
}

pub fn validate_api_token_scopes(scopes: Vec<String>) -> Result<Vec<String>, String> {
    const LEGACY_SCOPES: &[&str] = &[
        "stream:read",
        "system:read",
        "guest:connect",
        "phone:connect",
    ];
    if scopes.is_empty() || scopes.len() > 16 {
        return Err("API tokens require between 1 and 16 scopes".to_string());
    }
    let mut normalized = Vec::with_capacity(scopes.len());
    for scope in scopes {
        let scope = scope.trim().to_ascii_lowercase();
        if scope.is_empty()
            || !(scope == "*"
                || API_TOKEN_SCOPES.contains(&scope.as_str())
                || LEGACY_SCOPES.contains(&scope.as_str()))
        {
            return Err(format!(
                "Unsupported API token scope. Use one of: {}, *",
                API_TOKEN_SCOPES.join(", ")
            ));
        }
        normalized.push(scope);
    }
    normalized.sort();
    normalized.dedup();
    if normalized.iter().any(|scope| scope == "*") && normalized.len() != 1 {
        return Err("Wildcard scope must be used alone".to_string());
    }
    Ok(normalized)
}

fn api_token_authorized(ctx: &AuthContext, method: &str, path: &str) -> bool {
    let required = required_api_token_scope(method, path);
    scope_is_granted(ctx, required)
}

fn required_api_token_scope(method: &str, path: &str) -> &'static str {
    if path.starts_with("/api/auth/")
        || path == "/api/audit"
        || path.starts_with("/api/devices")
        || path.starts_with("/api/logs")
    {
        return "admin";
    }
    if path.starts_with("/api/license") {
        return "license";
    }
    if path.starts_with("/api/system") || path == "/api/ws" {
        return "system:read";
    }
    if path.starts_with("/api/stream") {
        return "stream:read";
    }
    if path.starts_with("/api/storage")
        || path.starts_with("/api/share")
        || path.starts_with("/api/projects/") && project_path_is_export(path)
    {
        return "export";
    }
    if path.starts_with("/api/cameras")
        || path.starts_with("/api/phones")
        || path.starts_with("/api/turntable")
        || path.starts_with("/api/hardware")
        || path.starts_with("/api/guest")
    {
        return if is_safe_method(method) {
            "read"
        } else {
            "capture"
        };
    }
    if path.starts_with("/api/jobs")
        || path.starts_with("/api/scan")
        || path.starts_with("/api/calibration")
        || path.starts_with("/api/wizard")
        || path.starts_with("/api/xr")
        || path.starts_with("/api/projects")
    {
        return if is_safe_method(method) {
            "read"
        } else {
            "process"
        };
    }
    if is_safe_method(method) {
        "read"
    } else {
        // Unknown mutations are privileged until explicitly classified.
        "admin"
    }
}

fn project_path_is_export(path: &str) -> bool {
    path.contains("/output/")
        || path.contains("/processed/")
        || path.contains("/fusion-artifact/")
        || path.ends_with("/assets")
}

fn system_time_to_secs(time: SystemTime) -> Result<i64, String> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Invalid system time".to_string())?;
    Ok(duration.as_secs() as i64)
}

pub struct AuthMiddleware {
    auth: Arc<AuthManager>,
    api_key: Option<String>,
    csrf_required: bool,
    rate_limiter: Option<Arc<RateLimiter>>,
    trusted_proxies: Arc<Vec<IpNet>>,
}

impl AuthMiddleware {
    pub fn new(
        auth: Arc<AuthManager>,
        api_key: Option<String>,
        csrf_required: bool,
        rate_limiter: Option<Arc<RateLimiter>>,
    ) -> Self {
        Self {
            auth,
            api_key,
            csrf_required,
            rate_limiter,
            trusted_proxies: Arc::new(Vec::new()),
        }
    }

    pub fn with_trusted_proxies(mut self, trusted_proxies: Arc<Vec<IpNet>>) -> Self {
        self.trusted_proxies = trusted_proxies;
        self
    }
}

impl<S, B> Transform<S, ServiceRequest> for AuthMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthMiddlewareService {
            service: Rc::new(service),
            auth: self.auth.clone(),
            api_key: self.api_key.clone(),
            csrf_required: self.csrf_required,
            rate_limiter: self.rate_limiter.clone(),
            trusted_proxies: self.trusted_proxies.clone(),
        }))
    }
}

pub struct AuthMiddlewareService<S> {
    service: Rc<S>,
    auth: Arc<AuthManager>,
    api_key: Option<String>,
    csrf_required: bool,
    rate_limiter: Option<Arc<RateLimiter>>,
    trusted_proxies: Arc<Vec<IpNet>>,
}

impl<S, B> Service<ServiceRequest> for AuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&self, ctx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(ctx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let auth = self.auth.clone();
        let api_key = self.api_key.clone();
        let csrf_required = self.csrf_required;
        let rate_limiter = self.rate_limiter.clone();
        let trusted_proxies = self.trusted_proxies.clone();
        let path = req.path().to_string();
        let method = req.method().to_string();

        if method == "OPTIONS" || is_public_path(&path) {
            let fut = self.service.call(req);
            return Box::pin(async move {
                let res = fut.await?;
                Ok(res.map_into_left_body())
            });
        }

        if let Some(limiter) = rate_limiter.as_ref() {
            if should_rate_limit(&path) {
                let ip_key = client_ip(req.request(), &trusted_proxies)
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let decision = limiter.check_ip(&ip_key);
                if !decision.allowed {
                    let mut resp = HttpResponse::TooManyRequests();
                    if let Some(retry_after) = decision.retry_after_seconds {
                        resp.insert_header((RETRY_AFTER, retry_after.to_string()));
                    }
                    let res = req.into_response(resp.body("Rate limit exceeded"));
                    return Box::pin(async move { Ok(res.map_into_right_body()) });
                }
            }
        }

        if let Some(key) = api_key.as_deref() {
            if header_matches_api_key(req.request(), key) {
                let service = self.service.clone();
                return Box::pin(async move {
                    let bootstrap_required = auth.bootstrap_required().await.unwrap_or(true);
                    if bootstrap_required && api_key_allowed_path(&path) {
                        req.extensions_mut().insert(AuthContext {
                            sub: "api_key".to_string(),
                            role: Role::Admin,
                            scopes: vec!["*".to_string()],
                            principal: PrincipalKind::BootstrapApiKey,
                        });
                        let res = service.call(req).await?;
                        return Ok(res.map_into_left_body());
                    }
                    let res = req.into_response(
                        HttpResponse::Forbidden().body("API key disabled after bootstrap"),
                    );
                    Ok(res.map_into_right_body())
                });
            }
        }

        if let Some(api_token) = extract_api_token(req.request()) {
            let service = self.service.clone();
            return Box::pin(async move {
                match auth.verify_api_token(&api_token).await {
                    Ok(ctx) if !api_token_authorized(&ctx, &method, &path) => {
                        let res = req.into_response(
                            HttpResponse::Forbidden().body("Insufficient API token scope"),
                        );
                        Ok(res.map_into_right_body())
                    }
                    Ok(ctx) if ctx.role == Role::Guest && !guest_allowed(&method, &path) => {
                        let res = req
                            .into_response(HttpResponse::Forbidden().body("Guest access denied"));
                        Ok(res.map_into_right_body())
                    }
                    Ok(ctx) => {
                        if let Some(limiter) = rate_limiter.as_ref() {
                            if should_rate_limit(&path) {
                                let decision = limiter.check_user(&ctx.sub);
                                if !decision.allowed {
                                    let mut resp = HttpResponse::TooManyRequests();
                                    if let Some(retry_after) = decision.retry_after_seconds {
                                        resp.insert_header((RETRY_AFTER, retry_after.to_string()));
                                    }
                                    let res = req.into_response(resp.body("Rate limit exceeded"));
                                    return Ok(res.map_into_right_body());
                                }
                            }
                        }
                        tracing::debug!(
                            principal = ?ctx.principal,
                            role = ?ctx.role,
                            method,
                            "request authenticated"
                        );
                        req.extensions_mut().insert(ctx);
                        let res = service.call(req).await?;
                        Ok(res.map_into_left_body())
                    }
                    Err(_) => {
                        let res = req
                            .into_response(HttpResponse::Unauthorized().body("Invalid API token"));
                        Ok(res.map_into_right_body())
                    }
                }
            });
        }

        let bearer = extract_bearer_token(req.request());
        let cookie_token = extract_cookie_token(req.request());
        let token = bearer.clone().or(cookie_token.clone());

        let token = match token {
            Some(t) => t,
            None => {
                let err = AuthError::Missing;
                tracing::warn!("{}", err);
                let res =
                    req.into_response(HttpResponse::Unauthorized().body("Missing auth token"));
                return Box::pin(async move { Ok(res.map_into_right_body()) });
            }
        };

        if bearer.is_none()
            && cookie_token.is_some()
            && csrf_required
            && !is_safe_method(&method)
            && !csrf_valid(req.request())
        {
            let res =
                req.into_response(HttpResponse::Forbidden().body("CSRF token missing or invalid"));
            return Box::pin(async move { Ok(res.map_into_right_body()) });
        }

        let service = self.service.clone();
        Box::pin(async move {
            let ctx = match auth.verify_token(&token).await {
                Ok(ctx) => ctx,
                Err(_) => {
                    let res =
                        req.into_response(HttpResponse::Unauthorized().body("Invalid auth token"));
                    return Ok(res.map_into_right_body());
                }
            };

            tracing::debug!(
                principal = ?ctx.principal,
                role = ?ctx.role,
                method,
                "request authenticated"
            );

            if ctx.role == Role::Guest && !guest_allowed(&method, &path) {
                let err = AuthError::NotAuthorized;
                tracing::warn!("{}", err);
                let res = req.into_response(HttpResponse::Forbidden().body("Guest access denied"));
                return Ok(res.map_into_right_body());
            }

            if let Some(limiter) = rate_limiter.as_ref() {
                if should_rate_limit(&path) {
                    let decision = limiter.check_user(&ctx.sub);
                    if !decision.allowed {
                        let mut resp = HttpResponse::TooManyRequests();
                        if let Some(retry_after) = decision.retry_after_seconds {
                            resp.insert_header((RETRY_AFTER, retry_after.to_string()));
                        }
                        let res = req.into_response(resp.body("Rate limit exceeded"));
                        return Ok(res.map_into_right_body());
                    }
                }
            }

            req.extensions_mut().insert(ctx);
            let res = service.call(req).await?;
            Ok(res.map_into_left_body())
        })
    }
}

fn extract_bearer_token(req: &HttpRequest) -> Option<String> {
    let header = req.headers().get(actix_web::http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_string())
}

pub const SESSION_COOKIE_NAME: &str = "trueshot_session";
pub const REFRESH_COOKIE_NAME: &str = "trueshot_refresh";
pub const CSRF_COOKIE_NAME: &str = "trueshot_csrf";
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";

fn extract_cookie_token(req: &HttpRequest) -> Option<String> {
    req.cookie(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
}

fn csrf_valid(req: &HttpRequest) -> bool {
    let cookie = match req.cookie(CSRF_COOKIE_NAME) {
        Some(cookie) => cookie.value().to_string(),
        None => return false,
    };
    let header = match req.headers().get(CSRF_HEADER_NAME) {
        Some(value) => value,
        None => return false,
    };
    let header_value = match header.to_str() {
        Ok(value) => value,
        Err(_) => return false,
    };
    cookie == header_value
}

fn is_safe_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS")
}

fn header_matches_api_key(req: &HttpRequest, expected: &str) -> bool {
    let header = match req.headers().get("X-API-Key") {
        Some(h) => h,
        None => return false,
    };
    let value = match header.to_str() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let supplied_digest = Sha256::digest(value.as_bytes());
    let expected_digest = Sha256::digest(expected.as_bytes());
    bool::from(supplied_digest.ct_eq(&expected_digest))
}

fn is_public_path(path: &str) -> bool {
    if path == "/api/health"
        || path == "/api/auth/pairing/claim"
        || path == "/api/auth/refresh"
        || path == "/api/auth/bootstrap/status"
        || path == "/api/auth/bootstrap"
        || path == "/api/auth/login"
        || path == "/api/public/shares"
        || path.starts_with("/share/")
        || path.starts_with("/s/")
    {
        return true;
    }
    if path.starts_with("/api/share/") {
        return !path.ends_with("/analytics") && !path.ends_with("/public");
    }
    false
}

fn should_rate_limit(path: &str) -> bool {
    if !path.starts_with("/api/") {
        return false;
    }
    !matches!(path, "/api/health" | "/api/metrics" | "/api/docs")
}

fn guest_allowed(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    path == "/api/health"
        || path.starts_with("/api/stream/")
        || path == "/api/ws"
        || path == "/api/system/stats"
}

fn api_key_allowed_path(path: &str) -> bool {
    path == "/api/auth/session"
}

fn extract_api_token(req: &HttpRequest) -> Option<String> {
    if let Some(header) = req.headers().get("X-API-Token") {
        if let Ok(value) = header.to_str() {
            if !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    let header = req.headers().get(actix_web::http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    if let Some(token) = value.strip_prefix("Token ") {
        if !token.trim().is_empty() {
            return Some(token.trim().to_string());
        }
    }
    None
}

fn client_ip(req: &HttpRequest, trusted_proxies: &[IpNet]) -> Option<IpAddr> {
    let peer = req.peer_addr()?.ip();
    if !trusted_proxies
        .iter()
        .any(|network| network.contains(&peer))
    {
        return Some(peer);
    }
    let forwarded = match req
        .headers()
        .get("X-Forwarded-For")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => value,
        None => return Some(peer),
    };
    let chain = match forwarded
        .split(',')
        .map(str::trim)
        .map(str::parse::<IpAddr>)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(chain) => chain,
        Err(_) => return Some(peer),
    };
    chain
        .iter()
        .rev()
        .copied()
        .find(|address| {
            !trusted_proxies
                .iter()
                .any(|network| network.contains(address))
        })
        .or(Some(peer))
}

pub(crate) fn configured_client_ip(
    req: &HttpRequest,
    trusted_proxy_cidrs: Option<&[String]>,
) -> Option<IpAddr> {
    let trusted_proxies = trusted_proxy_cidrs
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.parse::<IpNet>().ok())
        .collect::<Vec<_>>();
    client_ip(req, &trusted_proxies)
}

#[derive(Debug, Clone)]
pub struct PairingGrant {
    pub role: Role,
    pub scopes: Vec<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
struct PairingRate {
    tokens: u32,
    last_refill: Instant,
}

const PAIRING_TTL: Duration = Duration::from_secs(300);
const PAIRING_RATE_CAPACITY: u32 = 10;
const PAIRING_RATE_REFILL_SECS: u64 = 60;

fn refill_pairing_bucket(bucket: &mut PairingRate, now: Instant) {
    let elapsed = now.duration_since(bucket.last_refill).as_secs();
    if elapsed < PAIRING_RATE_REFILL_SECS {
        return;
    }
    let refill_steps = (elapsed / PAIRING_RATE_REFILL_SECS).min(u32::MAX as u64) as u32;
    let new_tokens = bucket
        .tokens
        .saturating_add(refill_steps.saturating_mul(PAIRING_RATE_CAPACITY));
    bucket.tokens = new_tokens.min(PAIRING_RATE_CAPACITY);
    bucket.last_refill = now;
}

fn generate_pairing_code() -> String {
    use rand::distributions::{Distribution, Uniform};

    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let distribution = Uniform::from(0..ALPHABET.len());
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| ALPHABET[distribution.sample(&mut rng)] as char)
        .collect()
}

fn generate_api_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64.encode(bytes)
}

fn hash_api_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_share_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn generate_short_code() -> String {
    use rand::distributions::{Distribution, Uniform};

    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let distribution = Uniform::from(0..ALPHABET.len());
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| ALPHABET[distribution.sample(&mut rng)] as char)
        .collect()
}

fn hash_share_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::Invalid)?
        .to_string();
    Ok(hash)
}

fn verify_password(hash: &str, password: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash).map_err(|_| AuthError::Invalid)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

fn login_identity_hash(email: &str) -> String {
    let normalized = email.trim().to_lowercase();
    let mut digest = Sha256::new();
    digest.update(b"trueshot-login-identity-v1\0");
    digest.update(normalized.as_bytes());
    hex::encode(digest.finalize())
}

fn dummy_password_hash() -> Result<&'static str, AuthError> {
    static DUMMY_HASH: OnceLock<Result<String, String>> = OnceLock::new();
    match DUMMY_HASH.get_or_init(|| {
        hash_password("trueshot-invalid-account-sentinel")
            .map_err(|error| format!("Initialize dummy password hash: {error}"))
    }) {
        Ok(hash) => Ok(hash),
        Err(error) => Err(AuthError::Storage(error.clone())),
    }
}

fn normalize_pairing_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn normalize_short_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn format_pairing_code(code: &str) -> String {
    let normalized = normalize_pairing_code(code);
    if normalized.len() <= 4 {
        return normalized;
    }
    let (left, right) = normalized.split_at(4);
    format!("{}-{}", left, right)
}

fn load_hmac_secret(configured_path: Option<&Path>, production: bool) -> Result<Vec<u8>, String> {
    if let Ok(encoded) = std::env::var(HMAC_SECRET_ENV) {
        return decode_hmac_secret(encoded.trim().as_bytes(), HMAC_SECRET_ENV);
    }
    let environment_path = std::env::var_os(HMAC_SECRET_FILE_ENV).map(std::path::PathBuf::from);
    if let Some(path) = environment_path.as_deref().or(configured_path) {
        return read_hmac_secret_file(path);
    }

    let entry = keyring::Entry::new("trueshot", HMAC_KEYRING_ENTRY)
        .map_err(|e| format!("Keyring init failed: {e}"))?;

    match entry.get_password() {
        Ok(encoded) => decode_hmac_secret(encoded.trim().as_bytes(), "keyring"),
        Err(err) => {
            if !matches!(err, keyring::Error::NoEntry) {
                return Err(format!("Keyring error: {err}"));
            }
            if production {
                return Err(format!(
                    "Persistent HMAC secret required in production. Set {HMAC_SECRET_ENV}, \
                     {HMAC_SECRET_FILE_ENV}, server.hmac_secret_path, or provision the OS keychain"
                ));
            }
            let mut secret = vec![0u8; MIN_HMAC_SECRET_BYTES];
            rand::thread_rng().fill_bytes(&mut secret);
            let encoded = B64.encode(&secret);
            entry
                .set_password(&encoded)
                .map_err(|e| format!("Failed to store key in keyring: {e}"))?;
            Ok(secret)
        }
    }
}

fn decode_hmac_secret(encoded: &[u8], source: &str) -> Result<Vec<u8>, String> {
    let decoded = B64
        .decode(encoded)
        .map_err(|error| format!("Invalid base64 HMAC secret from {source}: {error}"))?;
    validate_hmac_secret(decoded, source)
}

fn validate_hmac_secret(secret: Vec<u8>, source: &str) -> Result<Vec<u8>, String> {
    if !(MIN_HMAC_SECRET_BYTES..=MAX_HMAC_SECRET_BYTES).contains(&secret.len()) {
        return Err(format!(
            "HMAC secret from {source} must decode to between {MIN_HMAC_SECRET_BYTES} and \
             {MAX_HMAC_SECRET_BYTES} bytes"
        ));
    }
    Ok(secret)
}

fn read_hmac_secret_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Inspect HMAC secret file {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("HMAC secret path must be a regular non-symlink file".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("HMAC secret file permissions must be 0600 or stricter".to_string());
        }
        // SAFETY: `geteuid` has no preconditions and does not mutate memory.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid {
            return Err("HMAC secret file must be owned by the current user".to_string());
        }
    }
    if metadata.len() > MAX_HMAC_SECRET_BYTES as u64 * 2 {
        return Err("HMAC secret file is unexpectedly large".to_string());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Read HMAC secret file {}: {error}", path.display()))?;
    let trimmed = String::from_utf8_lossy(&bytes);
    if let Ok(decoded) = B64.decode(trimmed.trim().as_bytes()) {
        return validate_hmac_secret(decoded, &path.display().to_string());
    }
    validate_hmac_secret(bytes, &path.display().to_string())
}

fn decode_claims_with_secret(
    token: &str,
    issuer: &str,
    secret: &[u8],
) -> Result<Claims, AuthError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[issuer]);
    let data =
        decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation).map_err(|err| {
            match err.kind() {
                ErrorKind::ExpiredSignature => AuthError::Expired,
                _ => AuthError::Invalid,
            }
        })?;
    Ok(data.claims)
}

fn claims_to_context(claims: Claims) -> AuthContext {
    AuthContext {
        sub: claims.sub,
        role: claims.role,
        scopes: claims.scopes,
        principal: PrincipalKind::Session,
    }
}

#[derive(Debug, Clone)]
pub struct SessionTokens {
    pub subject: String,
    pub role: Role,
    pub access_token: String,
    pub refresh_token: String,
    pub csrf_token: String,
    pub access_ttl: Duration,
    pub refresh_ttl: Duration,
}

fn hash_refresh_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_BYTES: usize = 64;
    let mut key_block = [0u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK_BYTES];
    let mut outer_pad = [0x5cu8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn generate_refresh_token() -> String {
    const BYTES: usize = 32;
    let mut raw = [0u8; BYTES];
    rand::thread_rng().fill_bytes(&mut raw);
    B64.encode(raw)
}

fn generate_csrf_token() -> String {
    const BYTES: usize = 32;
    let mut raw = [0u8; BYTES];
    rand::thread_rng().fill_bytes(&mut raw);
    B64.encode(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_doc::ApiDoc;
    use actix_web::{http::StatusCode, test as actix_test, web, App};
    use futures::future::join_all;
    use std::collections::BTreeSet;
    use tempfile::TempDir;
    use utoipa::OpenApi;

    async fn test_auth() -> (TempDir, Arc<AuthStore>, Arc<AuthManager>) {
        let temp = tempfile::tempdir().expect("temporary auth directory");
        let store = Arc::new(
            AuthStore::new(&temp.path().join("auth.db"))
                .await
                .expect("auth store"),
        );
        let auth = Arc::new(AuthManager {
            issuer: "trueshot-test".to_string(),
            secret: vec![0x5a; 32],
            admin_ttl: Duration::from_secs(300),
            guest_ttl: Duration::from_secs(300),
            refresh_ttl: Duration::from_secs(600),
            store: store.clone(),
            pairing_rate: Mutex::new(HashMap::new()),
        });
        (temp, store, auth)
    }

    async fn insert_user(store: &AuthStore, id: &str, role: &str, active: bool) {
        store
            .create_user(&StoredUser {
                id: id.to_string(),
                email: format!("{id}@example.test"),
                name: id.to_string(),
                role: role.to_string(),
                password_hash: "not-used".to_string(),
                created_at: 1,
                last_login: None,
                active,
            })
            .await
            .expect("insert user");
    }

    async fn admin_guard(req: HttpRequest) -> HttpResponse {
        match require_admin(&req) {
            Ok(()) => HttpResponse::Ok().finish(),
            Err(response) => response,
        }
    }

    #[test]
    fn api_token_route_policy_is_fail_closed() {
        let cases = [
            ("GET", "/api/projects", "read"),
            ("GET", "/api/projects/p/output/a.tif", "export"),
            ("POST", "/api/projects/p/fusion-revisions", "process"),
            ("POST", "/api/cameras/c/capture", "capture"),
            ("GET", "/api/cameras", "read"),
            ("POST", "/api/license/activate", "license"),
            ("GET", "/api/auth/tokens", "admin"),
            ("GET", "/api/system/stats", "system:read"),
            ("GET", "/api/stream/camera", "stream:read"),
            ("POST", "/api/new-unclassified-operation", "admin"),
        ];
        for (method, path, expected) in cases {
            assert_eq!(required_api_token_scope(method, path), expected);
        }
    }

    #[test]
    fn api_token_scope_validation_is_bounded_and_unambiguous() {
        assert_eq!(
            validate_api_token_scopes(vec!["process".to_string(), "read".to_string()])
                .expect("valid scopes"),
            vec!["process".to_string(), "read".to_string()]
        );
        assert!(validate_api_token_scopes(vec!["*".to_string(), "read".to_string()]).is_err());
        assert!(validate_api_token_scopes(vec!["unknown".to_string()]).is_err());
        assert!(validate_api_token_scopes(Vec::new()).is_err());
        let canonical_read = AuthContext {
            sub: "reader".to_string(),
            role: Role::Admin,
            scopes: vec!["read".to_string()],
            principal: PrincipalKind::ApiToken,
        };
        assert!(scope_is_granted(&canonical_read, "system:read"));
        assert!(scope_is_granted(&canonical_read, "stream:read"));
    }

    #[test]
    fn bootstrap_api_key_comparison_matches_only_the_exact_secret() {
        let matching = actix_test::TestRequest::default()
            .insert_header(("X-API-Key", "bootstrap-secret"))
            .to_http_request();
        let wrong = actix_test::TestRequest::default()
            .insert_header(("X-API-Key", "bootstrap-secreu"))
            .to_http_request();
        assert!(header_matches_api_key(&matching, "bootstrap-secret"));
        assert!(!header_matches_api_key(&wrong, "bootstrap-secret"));
    }

    #[test]
    fn share_alias_hmac_matches_rfc_4231_sha256_vector() {
        assert_eq!(
            hex::encode(hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hmac_secret_file_requires_private_regular_file_and_strong_material() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hmac.key");
        let secret = vec![0x4du8; MIN_HMAC_SECRET_BYTES];
        std::fs::write(&path, B64.encode(&secret)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_hmac_secret_file(&path).unwrap(), secret);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_hmac_secret_file(&path).unwrap_err().contains("0600"));

        let weak = temp.path().join("weak.key");
        std::fs::write(&weak, B64.encode([1u8; MIN_HMAC_SECRET_BYTES - 1])).unwrap();
        std::fs::set_permissions(&weak, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_hmac_secret_file(&weak)
            .unwrap_err()
            .contains("must decode"));
    }

    #[test]
    fn forwarded_client_ip_is_used_only_across_explicitly_trusted_proxies() {
        let trusted = vec!["10.0.0.0/8".parse::<IpNet>().unwrap()];
        let spoofed = actix_test::TestRequest::default()
            .peer_addr("203.0.113.9:4321".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.7"))
            .to_http_request();
        assert_eq!(
            client_ip(&spoofed, &trusted),
            Some("203.0.113.9".parse().unwrap())
        );

        let proxied = actix_test::TestRequest::default()
            .peer_addr("10.0.0.3:443".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.7, 10.0.0.2"))
            .to_http_request();
        assert_eq!(
            client_ip(&proxied, &trusted),
            Some("198.51.100.7".parse().unwrap())
        );

        let malformed = actix_test::TestRequest::default()
            .peer_addr("10.0.0.3:443".parse().unwrap())
            .insert_header(("X-Forwarded-For", "not-an-ip"))
            .to_http_request();
        assert_eq!(
            client_ip(&malformed, &trusted),
            Some("10.0.0.3".parse().unwrap())
        );
    }

    #[test]
    fn documented_route_principal_scope_matrix_is_complete_and_fail_closed() {
        const METHODS: &[&str] = &[
            "get", "head", "post", "put", "patch", "delete", "options", "trace",
        ];
        const TOKEN_SCOPES: &[&str] = &[
            "read",
            "capture",
            "process",
            "export",
            "license",
            "admin",
            "system:read",
            "stream:read",
        ];
        let document = serde_json::to_value(ApiDoc::openapi()).expect("serialize OpenAPI");
        let paths = document
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("OpenAPI paths");
        let mut route_count = 0usize;
        let mut public_count = 0usize;
        let mut documented_routes = BTreeSet::new();
        for (path, item) in paths {
            let operations = item.as_object().expect("OpenAPI path item");
            for (method, _) in operations {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                route_count += 1;
                let method = method.to_ascii_uppercase();
                documented_routes.insert((method.clone(), path.clone()));
                if is_public_path(path) {
                    public_count += 1;
                    continue;
                }

                let required = required_api_token_scope(&method, path);
                assert!(
                    TOKEN_SCOPES.contains(&required),
                    "{method} {path} resolved to unsupported scope {required}"
                );
                let wildcard = AuthContext {
                    sub: "wildcard".to_string(),
                    role: Role::Admin,
                    scopes: vec!["*".to_string()],
                    principal: PrincipalKind::ApiToken,
                };
                assert!(
                    api_token_authorized(&wildcard, &method, path),
                    "wildcard token rejected for {method} {path}"
                );

                for scope in TOKEN_SCOPES {
                    let context = AuthContext {
                        sub: format!("{scope}-token"),
                        role: Role::Admin,
                        scopes: vec![(*scope).to_string()],
                        principal: PrincipalKind::ApiToken,
                    };
                    let expected = *scope == "admin"
                        || *scope == required
                        || (required.ends_with(":read") && *scope == "read");
                    assert_eq!(
                        api_token_authorized(&context, &method, path),
                        expected,
                        "scope {scope} classification mismatch for {method} {path}; required {required}"
                    );
                }

                let guest_session = AuthContext {
                    sub: "guest".to_string(),
                    role: Role::Guest,
                    scopes: vec!["read".to_string()],
                    principal: PrincipalKind::Session,
                };
                assert_eq!(
                    guest_allowed(&method, path),
                    method == "GET"
                        && (matches!(
                            path.as_str(),
                            "/api/health" | "/api/ws" | "/api/system/stats"
                        ) || path.starts_with("/api/stream/")),
                    "guest route policy drift for {method} {path}"
                );
                assert_eq!(guest_session.role, Role::Guest);
            }
        }
        assert!(
            route_count >= 140,
            "OpenAPI route inventory unexpectedly shrank to {route_count}"
        );
        assert!(
            public_count >= 8,
            "public route inventory unexpectedly shrank to {public_count}"
        );

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut declared_routes = BTreeSet::new();
        for source_root in [manifest_dir.join("src/api"), manifest_dir.join("src/guest")] {
            for entry in walkdir::WalkDir::new(source_root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry.path().extension().and_then(|value| value.to_str()) == Some("rs")
                })
            {
                let source = std::fs::read_to_string(entry.path()).expect("read route source");
                for line in source.lines() {
                    let line = line.trim();
                    for method in METHODS {
                        let prefix = format!("#[{method}(\"");
                        if let Some(rest) = line.strip_prefix(&prefix) {
                            if let Some(path) = rest.split('"').next() {
                                let mut normalized = String::with_capacity(path.len());
                                let mut in_parameter = false;
                                let mut in_parameter_pattern = false;
                                for character in path.chars() {
                                    match character {
                                        '{' => {
                                            in_parameter = true;
                                            normalized.push(character);
                                        }
                                        ':' if in_parameter => {
                                            in_parameter_pattern = true;
                                        }
                                        '}' => {
                                            in_parameter = false;
                                            in_parameter_pattern = false;
                                            normalized.push(character);
                                        }
                                        _ if !in_parameter_pattern => normalized.push(character),
                                        _ => {}
                                    }
                                }
                                declared_routes.insert((method.to_ascii_uppercase(), normalized));
                            }
                        }
                    }
                }
            }
        }
        let undocumented: Vec<_> = declared_routes
            .difference(&documented_routes)
            .cloned()
            .collect();
        let expected_browser_routes = BTreeSet::from([
            ("GET".to_string(), "/s/{code}".to_string()),
            ("GET".to_string(), "/share/{token}/card".to_string()),
        ]);
        assert_eq!(
            undocumented.into_iter().collect::<BTreeSet<_>>(),
            expected_browser_routes,
            "Actix routes missing from the generated principal/scope matrix"
        );
        for (_, path) in expected_browser_routes {
            assert!(
                is_public_path(&path),
                "browser route must remain public: {path}"
            );
        }
    }

    #[actix_web::test]
    async fn bootstrap_api_key_is_async_and_expires_after_admin_creation() {
        let (_temp, store, auth) = test_auth().await;
        let app = actix_test::init_service(
            App::new()
                .wrap(AuthMiddleware::new(
                    auth,
                    Some("bootstrap-secret".to_string()),
                    false,
                    None,
                ))
                .route("/api/auth/session", web::post().to(admin_guard)),
        )
        .await;

        let request = actix_test::TestRequest::post()
            .uri("/api/auth/session")
            .insert_header(("X-API-Key", "bootstrap-secret"))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::OK
        );

        insert_user(&store, "admin", "Admin", true).await;
        let request = actix_test::TestRequest::post()
            .uri("/api/auth/session")
            .insert_header(("X-API-Key", "bootstrap-secret"))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::FORBIDDEN
        );
    }

    #[actix_web::test]
    async fn api_token_scopes_gate_routes_under_concurrent_actix_requests() {
        let (_temp, store, auth) = test_auth().await;
        insert_user(&store, "admin", "Admin", true).await;
        let (read_token, _) = auth
            .create_api_token("admin", "read", vec!["read".to_string()], None)
            .await
            .expect("read token");
        let (process_token, _) = auth
            .create_api_token("admin", "process", vec!["process".to_string()], None)
            .await
            .expect("process token");

        let app = actix_test::init_service(
            App::new()
                .wrap(AuthMiddleware::new(auth, None, false, None))
                .route("/api/projects/example", web::get().to(admin_guard))
                .route("/api/projects/example", web::post().to(admin_guard)),
        )
        .await;

        let read_requests = (0..16).map(|_| {
            let request = actix_test::TestRequest::get()
                .uri("/api/projects/example")
                .insert_header(("X-API-Token", read_token.clone()))
                .to_request();
            actix_test::call_service(&app, request)
        });
        for response in join_all(read_requests).await {
            assert_eq!(response.status(), StatusCode::OK);
        }

        let request = actix_test::TestRequest::post()
            .uri("/api/projects/example")
            .insert_header(("X-API-Token", read_token))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::FORBIDDEN
        );

        let request = actix_test::TestRequest::post()
            .uri("/api/projects/example")
            .insert_header(("X-API-Token", process_token))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::OK
        );
    }

    #[actix_web::test]
    async fn api_token_verification_rejects_invalid_owner_and_token_state() {
        let (_temp, store, auth) = test_auth().await;
        insert_user(&store, "inactive", "Admin", true).await;
        let (inactive_token, _) = auth
            .create_api_token("inactive", "inactive", vec!["read".to_string()], None)
            .await
            .expect("inactive-owner token");
        store
            .set_user_active_for_test("inactive", false)
            .await
            .expect("deactivate owner");
        assert!(matches!(
            auth.verify_api_token(&inactive_token).await,
            Err(AuthError::NotAuthorized)
        ));

        insert_user(&store, "admin", "Admin", true).await;
        let (revoked_token, revoked) = auth
            .create_api_token("admin", "revoked", vec!["read".to_string()], None)
            .await
            .expect("revoked token");
        auth.revoke_api_token(&revoked.id)
            .await
            .expect("revoke token");
        assert!(matches!(
            auth.verify_api_token(&revoked_token).await,
            Err(AuthError::Expired)
        ));

        let (expired_token, _) = auth
            .create_api_token("admin", "expired", vec!["read".to_string()], Some(1))
            .await
            .expect("expired token");
        assert!(matches!(
            auth.verify_api_token(&expired_token).await,
            Err(AuthError::Expired)
        ));

        assert!(matches!(
            auth.create_api_token("missing", "orphan", vec!["read".to_string()], None)
                .await,
            Err(AuthError::Invalid)
        ));

        let (storage_token, _) = auth
            .create_api_token("admin", "storage", vec!["read".to_string()], None)
            .await
            .expect("storage token");
        let app = actix_test::init_service(
            App::new()
                .wrap(AuthMiddleware::new(auth.clone(), None, false, None))
                .route("/api/projects/example", web::get().to(admin_guard)),
        )
        .await;
        store.close_for_test().await;
        assert!(matches!(
            auth.verify_api_token("unavailable").await,
            Err(AuthError::Storage(_))
        ));
        let request = actix_test::TestRequest::get()
            .uri("/api/projects/example")
            .insert_header(("X-API-Token", storage_token))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[actix_web::test]
    async fn login_backoff_is_atomic_progressive_and_window_bounded() {
        let (_temp, store, _auth) = test_auth().await;
        let identity = login_identity_hash("operator@example.test");
        let mut throttle = None;
        for attempt in 1..=LOGIN_FAILURE_THRESHOLD {
            let observed = store
                .record_login_failure(
                    &identity,
                    1_000,
                    LOGIN_FAILURE_WINDOW_SECONDS,
                    LOGIN_FAILURE_THRESHOLD,
                    LOGIN_BASE_LOCK_SECONDS,
                    LOGIN_MAX_LOCK_SECONDS,
                )
                .await
                .expect("record failure");
            assert_eq!(observed.failed_attempts, attempt);
            throttle = Some(observed);
        }
        let throttle = throttle.expect("throttle");
        assert_eq!(throttle.locked_until, 1_000 + LOGIN_BASE_LOCK_SECONDS);

        store
            .set_login_lock_for_test(&identity, 0)
            .await
            .expect("expire first lock");
        let progressive = store
            .record_login_failure(
                &identity,
                1_031,
                LOGIN_FAILURE_WINDOW_SECONDS,
                LOGIN_FAILURE_THRESHOLD,
                LOGIN_BASE_LOCK_SECONDS,
                LOGIN_MAX_LOCK_SECONDS,
            )
            .await
            .expect("record progressive failure");
        assert_eq!(progressive.failed_attempts, LOGIN_FAILURE_THRESHOLD + 1);
        assert_eq!(
            progressive.locked_until,
            1_031 + LOGIN_BASE_LOCK_SECONDS * 2
        );

        let reset = store
            .record_login_failure(
                &identity,
                1_000 + LOGIN_FAILURE_WINDOW_SECONDS,
                LOGIN_FAILURE_WINDOW_SECONDS,
                LOGIN_FAILURE_THRESHOLD,
                LOGIN_BASE_LOCK_SECONDS,
                LOGIN_MAX_LOCK_SECONDS,
            )
            .await
            .expect("reset expired window");
        assert_eq!(reset.failed_attempts, 1);
        assert_eq!(reset.window_started, 1_000 + LOGIN_FAILURE_WINDOW_SECONDS);
        assert_eq!(reset.locked_until, 0);
    }

    #[actix_web::test]
    async fn password_lockout_survives_manager_restart_and_success_clears_it() {
        let (_temp, store, auth) = test_auth().await;
        let user = auth
            .create_admin_user(
                "Operator@Example.Test",
                "Operator",
                "correct horse battery staple",
            )
            .await
            .expect("create operator");
        assert_eq!(user.email, "operator@example.test");

        for attempt in 1..=LOGIN_FAILURE_THRESHOLD {
            let result = auth
                .verify_password_login("operator@example.test", "incorrect password")
                .await;
            if attempt < LOGIN_FAILURE_THRESHOLD {
                assert!(matches!(result, Err(AuthError::Invalid)));
            } else {
                let Err(AuthError::RateLimited {
                    retry_after_seconds,
                }) = result
                else {
                    panic!("threshold failure must activate durable lockout");
                };
                assert!(retry_after_seconds >= LOGIN_BASE_LOCK_SECONDS as u64);
            }
        }

        let restarted = AuthManager {
            issuer: "trueshot-test-restarted".to_string(),
            secret: vec![0x6b; 32],
            admin_ttl: Duration::from_secs(300),
            guest_ttl: Duration::from_secs(300),
            refresh_ttl: Duration::from_secs(600),
            store: store.clone(),
            pairing_rate: Mutex::new(HashMap::new()),
        };
        assert!(matches!(
            restarted
                .verify_password_login("OPERATOR@EXAMPLE.TEST", "correct horse battery staple")
                .await,
            Err(AuthError::RateLimited { .. })
        ));

        let identity = login_identity_hash("operator@example.test");
        store
            .set_login_lock_for_test(&identity, 0)
            .await
            .expect("expire lock");
        let logged_in = restarted
            .verify_password_login("OPERATOR@EXAMPLE.TEST", "correct horse battery staple")
            .await
            .expect("valid login after lock expiry");
        assert_eq!(logged_in.id, user.id);
        assert!(
            store
                .get_login_throttle(&identity)
                .await
                .expect("read throttle")
                .is_none(),
            "successful login must clear persisted failure state"
        );
    }

    #[actix_web::test]
    async fn access_token_jti_and_subject_revocation_survive_restart() {
        let (_temp, store, auth) = test_auth().await;
        let first = auth
            .issue_session_tokens("operator", Role::Admin, vec!["*".to_string()])
            .await
            .expect("first session");
        let second = auth
            .issue_session_tokens("operator", Role::Admin, vec!["*".to_string()])
            .await
            .expect("second session");
        let unrelated = auth
            .issue_session_tokens("other", Role::Admin, vec!["*".to_string()])
            .await
            .expect("unrelated session");
        assert!(auth.verify_token(&first.access_token).await.is_ok());
        assert!(auth.verify_token(&second.access_token).await.is_ok());

        let first_claims =
            decode_claims_with_secret(&first.access_token, "trueshot-test", &[0x5a; 32])
                .expect("decode first claims");
        let second_claims =
            decode_claims_with_secret(&second.access_token, "trueshot-test", &[0x5a; 32])
                .expect("decode second claims");
        assert_ne!(first_claims.jti, second_claims.jti);
        assert_eq!(first_claims.session_version, 0);

        auth.revoke_access_token(&first.access_token)
            .await
            .expect("revoke first JTI");
        assert!(matches!(
            auth.verify_token(&first.access_token).await,
            Err(AuthError::Expired)
        ));
        assert!(auth.verify_token(&second.access_token).await.is_ok());

        let restarted = AuthManager {
            issuer: "trueshot-test".to_string(),
            secret: vec![0x5a; 32],
            admin_ttl: Duration::from_secs(300),
            guest_ttl: Duration::from_secs(300),
            refresh_ttl: Duration::from_secs(600),
            store: store.clone(),
            pairing_rate: Mutex::new(HashMap::new()),
        };
        assert!(matches!(
            restarted.verify_token(&first.access_token).await,
            Err(AuthError::Expired)
        ));
        assert!(restarted.verify_token(&second.access_token).await.is_ok());

        restarted
            .revoke_all_for_subject("operator")
            .await
            .expect("revoke subject generation");
        assert!(matches!(
            restarted.verify_token(&second.access_token).await,
            Err(AuthError::Expired)
        ));
        assert!(restarted
            .verify_token(&unrelated.access_token)
            .await
            .is_ok());
        assert!(matches!(
            restarted.refresh_session(&second.refresh_token).await,
            Err(AuthError::Expired)
        ));

        let replacement = restarted
            .issue_session_tokens("operator", Role::Admin, vec!["*".to_string()])
            .await
            .expect("replacement session");
        let replacement_claims =
            decode_claims_with_secret(&replacement.access_token, "trueshot-test", &[0x5a; 32])
                .expect("decode replacement claims");
        assert_eq!(replacement_claims.session_version, 1);
        assert!(restarted
            .verify_token(&replacement.access_token)
            .await
            .is_ok());
        assert!(restarted
            .refresh_session(&replacement.refresh_token)
            .await
            .is_ok());
    }

    #[actix_web::test]
    async fn middleware_rejects_persistently_revoked_access_token() {
        let (_temp, _store, auth) = test_auth().await;
        let session = auth
            .issue_session_tokens("operator", Role::Admin, vec!["*".to_string()])
            .await
            .expect("session");
        let app = actix_test::init_service(
            App::new()
                .wrap(AuthMiddleware::new(auth.clone(), None, false, None))
                .route("/api/projects/example", web::get().to(admin_guard)),
        )
        .await;
        let request = actix_test::TestRequest::get()
            .uri("/api/projects/example")
            .insert_header(("Authorization", format!("Bearer {}", session.access_token)))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::OK
        );

        auth.revoke_access_token(&session.access_token)
            .await
            .expect("revoke access token");
        let request = actix_test::TestRequest::get()
            .uri("/api/projects/example")
            .insert_header(("Authorization", format!("Bearer {}", session.access_token)))
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, request).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[actix_web::test]
    async fn refresh_cannot_cross_a_concurrent_subject_generation_change() {
        let (_temp, store, auth) = test_auth().await;
        let session = auth
            .issue_session_tokens("operator", Role::Admin, vec!["*".to_string()])
            .await
            .expect("session");
        let now = system_time_to_secs(SystemTime::now()).expect("current time");
        store
            .increment_access_subject_version("operator", now)
            .await
            .expect("advance generation without deleting refresh");
        assert!(matches!(
            auth.refresh_session(&session.refresh_token).await,
            Err(AuthError::Expired)
        ));
    }

    #[actix_web::test]
    async fn public_share_alias_migration_scrubs_raw_bearer_and_preserves_access() {
        let (directory, store, auth) = test_auth().await;
        let (raw_token, link) = auth
            .create_share_link("project", "output/model.glb", 3_600, None, true, true)
            .await
            .expect("share link");
        let public = auth
            .upsert_share_public(
                &raw_token,
                true,
                Some("Model".to_string()),
                None,
                vec!["test".to_string()],
                None,
                Some("model".to_string()),
            )
            .await
            .expect("public share");
        let alias = auth.public_share_alias_for_hash(&link.token_hash);
        assert_ne!(alias, raw_token);
        assert_eq!(public.public_alias_hash, hash_share_token(&alias));
        assert_eq!(
            store
                .public_token_storage_for_test(&link.token_hash)
                .await
                .expect("storage")
                .expect("public row")
                .0,
            ""
        );
        assert!(auth
            .get_share_link(&alias)
            .await
            .expect("alias lookup")
            .is_some());
        assert!(auth
            .get_share_public(&alias)
            .await
            .expect("alias metadata")
            .is_some());

        store
            .restore_legacy_public_token_for_test(&link.token_hash, &raw_token)
            .await
            .expect("restore legacy leak");
        assert_eq!(
            auth.migrate_public_share_aliases().await.expect("migrate"),
            1
        );
        let (stored_raw, stored_alias_hash) = store
            .public_token_storage_for_test(&link.token_hash)
            .await
            .expect("storage")
            .expect("public row");
        assert!(stored_raw.is_empty());
        assert_eq!(
            stored_alias_hash.as_deref(),
            Some(hash_share_token(&alias).as_str())
        );

        for suffix in ["", "-wal", "-shm", "-journal"] {
            let path = directory.path().join(format!("auth.db{suffix}"));
            if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !bytes
                        .windows(raw_token.len())
                        .any(|window| window == raw_token.as_bytes()),
                    "legacy bearer survived secure scrub in {}",
                    path.display()
                );
            }
        }

        auth.record_share_access(
            &alias,
            "asset",
            system_time_to_secs(SystemTime::now()).expect("time"),
            None,
            None,
            None,
            false,
            false,
        )
        .await
        .expect("record alias access");
        assert_eq!(
            auth.get_share_analytics(&alias)
                .await
                .expect("alias analytics")
                .asset_requests,
            1
        );
    }
}
