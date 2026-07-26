use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::RETRY_AFTER;
use actix_web::{body::EitherBody, Error, HttpMessage, HttpRequest, HttpResponse};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use jsonwebtoken::{
    decode, encode, errors::ErrorKind, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::{ready, Ready};
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::task::block_in_place;

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
    pub exp: usize,
    pub iat: usize,
    pub iss: String,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub sub: String,
    pub role: Role,
    pub scopes: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum AuthError {
    Missing,
    Invalid,
    Expired,
    NotAuthorized,
    KeychainUnavailable(String),
    RateLimited,
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
            AuthError::RateLimited => write!(f, "Rate limited"),
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
    pub fn new(
        issuer: String,
        admin_ttl: Duration,
        guest_ttl: Duration,
        refresh_ttl: Duration,
        store: Arc<AuthStore>,
    ) -> Result<Self, AuthError> {
        let secret = load_or_create_hmac_secret().map_err(AuthError::KeychainUnavailable)?;
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

    pub fn issue_admin_token(
        &self,
        subject: &str,
        scopes: Vec<String>,
    ) -> Result<String, AuthError> {
        self.issue_token(subject, Role::Admin, scopes, self.admin_ttl)
    }

    pub fn issue_guest_token(
        &self,
        subject: &str,
        scopes: Vec<String>,
    ) -> Result<String, AuthError> {
        self.issue_token(subject, Role::Guest, scopes, self.guest_ttl)
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

    pub fn refresh_ttl_seconds(&self) -> u64 {
        self.refresh_ttl.as_secs()
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
        let password_hash = hash_password(password)?;
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let user = StoredUser {
            id: uuid::Uuid::new_v4().to_string(),
            email: email.to_string(),
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
        let user = self
            .store
            .get_user_by_email(email)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?
            .ok_or(AuthError::Invalid)?;
        if !user.active {
            return Err(AuthError::NotAuthorized);
        }
        if !verify_password(&user.password_hash, password)? {
            return Err(AuthError::Invalid);
        }
        let now = system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?;
        let _ = self.store.update_user_last_login(&user.id, now).await;
        Ok(user)
    }

    pub async fn create_api_token(
        &self,
        user_id: &str,
        name: &str,
        scopes: Vec<String>,
        expires_at: Option<i64>,
    ) -> Result<(String, StoredApiToken), AuthError> {
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
        let _ = self
            .store
            .touch_api_token(
                &token.id,
                system_time_to_secs(SystemTime::now()).map_err(AuthError::Storage)?,
            )
            .await;
        Ok(AuthContext {
            sub: token.user_id,
            role: Role::Admin,
            scopes: token.scopes,
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
        let hash = hash_share_token(token);
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
        let hash = hash_share_token(token);
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
        let hash = hash_share_token(token);
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
        let hash = hash_share_token(token);
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
            token_hash: hash,
            public_token: token.to_string(),
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
        self.store
            .get_share_public(&hash)
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

    pub fn verify_token(&self, token: &str) -> Result<AuthContext, AuthError> {
        verify_with_secret(token, &self.issuer, &self.secret)
    }

    pub async fn issue_session_tokens(
        &self,
        subject: &str,
        role: Role,
        scopes: Vec<String>,
    ) -> Result<SessionTokens, AuthError> {
        let access_ttl = match role {
            Role::Admin => self.admin_ttl,
            Role::Guest => self.guest_ttl,
        };
        let access_token = self.issue_token(subject, role, scopes.clone(), access_ttl)?;
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
        let _ = self.store.delete_refresh_session(&refresh_hash).await;
        let role = match entry.role.as_str() {
            "Admin" => Role::Admin,
            _ => Role::Guest,
        };
        self.issue_session_tokens(&entry.subject, role, entry.scopes)
            .await
    }

    pub async fn revoke_refresh_token(&self, refresh_token: &str) {
        let refresh_hash = hash_refresh_token(refresh_token);
        let _ = self.store.delete_refresh_session(&refresh_hash).await;
    }

    pub async fn revoke_all_for_subject(&self, subject: &str) {
        let _ = self
            .store
            .delete_refresh_sessions_for_subject(subject)
            .await;
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
                return Err(AuthError::RateLimited);
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

    fn allow_pairing_attempt(&self, ip: IpAddr) -> bool {
        let mut buckets = self.pairing_rate.lock().unwrap();
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
        Some(ctx) if ctx.role == Role::Admin => Ok(()),
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
            if ctx.role == Role::Admin {
                return Ok(());
            }
            if ctx.scopes.iter().any(|s| s == "*" || s == scope) {
                Ok(())
            } else {
                Err(HttpResponse::Forbidden().body("Insufficient scope"))
            }
        }
        None => Err(HttpResponse::Unauthorized().body("Unauthorized")),
    }
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
        }
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
            service,
            auth: self.auth.clone(),
            api_key: self.api_key.clone(),
            csrf_required: self.csrf_required,
            rate_limiter: self.rate_limiter.clone(),
        }))
    }
}

pub struct AuthMiddlewareService<S> {
    service: S,
    auth: Arc<AuthManager>,
    api_key: Option<String>,
    csrf_required: bool,
    rate_limiter: Option<Arc<RateLimiter>>,
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
                let ip_key = client_ip(req.request())
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
                let bootstrap_required = block_in_place(|| {
                    Handle::current()
                        .block_on(auth.bootstrap_required())
                        .unwrap_or(true)
                });
                if bootstrap_required && api_key_allowed_path(&path) {
                    req.extensions_mut().insert(AuthContext {
                        sub: "api_key".to_string(),
                        role: Role::Admin,
                        scopes: vec!["*".to_string()],
                    });
                    let fut = self.service.call(req);
                    return Box::pin(async move {
                        let res = fut.await?;
                        Ok(res.map_into_left_body())
                    });
                }
                let res = req.into_response(
                    HttpResponse::Forbidden().body("API key disabled after bootstrap"),
                );
                return Box::pin(async move { Ok(res.map_into_right_body()) });
            }
        }

        if let Some(api_token) = extract_api_token(req.request()) {
            let ctx =
                block_in_place(|| Handle::current().block_on(auth.verify_api_token(&api_token)));
            match ctx {
                Ok(ctx) => {
                    req.extensions_mut().insert(ctx);
                    let fut = self.service.call(req);
                    return Box::pin(async move {
                        let res = fut.await?;
                        Ok(res.map_into_left_body())
                    });
                }
                Err(_) => {
                    let res =
                        req.into_response(HttpResponse::Unauthorized().body("Invalid API token"));
                    return Box::pin(async move { Ok(res.map_into_right_body()) });
                }
            }
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

        let ctx = match auth.verify_token(&token) {
            Ok(ctx) => ctx,
            Err(_) => {
                let res =
                    req.into_response(HttpResponse::Unauthorized().body("Invalid auth token"));
                return Box::pin(async move { Ok(res.map_into_right_body()) });
            }
        };

        tracing::debug!(
            "Auth subject={} role={:?} scopes={:?} method={} path={}",
            ctx.sub,
            ctx.role,
            ctx.scopes,
            method,
            path
        );

        if ctx.role == Role::Guest && !guest_allowed(&method, &path) {
            let err = AuthError::NotAuthorized;
            tracing::warn!("{}", err);
            let res = req.into_response(HttpResponse::Forbidden().body("Guest access denied"));
            return Box::pin(async move { Ok(res.map_into_right_body()) });
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
                    return Box::pin(async move { Ok(res.map_into_right_body()) });
                }
            }
        }

        req.extensions_mut().insert(ctx);
        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await?;
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
    value == expected
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

fn client_ip(req: &HttpRequest) -> Option<IpAddr> {
    req.connection_info()
        .realip_remote_addr()
        .and_then(|addr| addr.split(',').next())
        .and_then(|addr| addr.trim().parse().ok())
        .or_else(|| req.peer_addr().map(|addr| addr.ip()))
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
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
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
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
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

fn load_or_create_hmac_secret() -> Result<Vec<u8>, String> {
    let entry = keyring::Entry::new("trueshot", "server_hmac_secret")
        .map_err(|e| format!("Keyring init failed: {e}"))?;

    match entry.get_password() {
        Ok(encoded) => {
            let decoded = B64
                .decode(encoded.as_bytes())
                .map_err(|e| format!("Invalid key material in keyring: {e}"))?;
            if decoded.len() < 32 {
                return Err("Keyring secret too short".to_string());
            }
            Ok(decoded)
        }
        Err(err) => {
            if !matches!(err, keyring::Error::NoEntry) {
                return Err(format!("Keyring error: {err}"));
            }
            let mut secret = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut secret);
            let encoded = B64.encode(&secret);
            entry
                .set_password(&encoded)
                .map_err(|e| format!("Failed to store key in keyring: {e}"))?;
            Ok(secret)
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthVerifier {
    issuer: String,
    secret: Vec<u8>,
}

impl AuthVerifier {
    pub fn new(issuer: impl Into<String>) -> Result<Self, AuthError> {
        let secret = load_or_create_hmac_secret().map_err(AuthError::KeychainUnavailable)?;
        Ok(Self {
            issuer: issuer.into(),
            secret,
        })
    }

    pub fn verify_token(&self, token: &str) -> Result<AuthContext, AuthError> {
        verify_with_secret(token, &self.issuer, &self.secret)
    }
}

fn verify_with_secret(token: &str, issuer: &str, secret: &[u8]) -> Result<AuthContext, AuthError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[issuer]);
    let data =
        decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation).map_err(|err| {
            match err.kind() {
                ErrorKind::ExpiredSignature => AuthError::Expired,
                _ => AuthError::Invalid,
            }
        })?;
    Ok(AuthContext {
        sub: data.claims.sub,
        role: data.claims.role,
        scopes: data.claims.scopes,
    })
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
