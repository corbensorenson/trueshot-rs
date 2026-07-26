use actix_web::cookie::{time::Duration as CookieDuration, Cookie, SameSite};
use actix_web::{delete, get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::AuditEvent;
use crate::auth::{
    require_admin, AuthContext, Role, CSRF_COOKIE_NAME, CSRF_HEADER_NAME, REFRESH_COOKIE_NAME,
    SESSION_COOKIE_NAME,
};
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    pub token: String,
    pub role: Role,
    pub expires_in_seconds: u64,
    pub refresh_expires_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GuestRequest {
    pub label: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/auth/guest",
    tag = "auth",
    request_body = GuestRequest,
    responses(
        (status = 200, description = "Guest token issued", body = TokenResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
#[post("/api/auth/guest")]
pub async fn guest_token(
    req: HttpRequest,
    state: web::Data<AppState>,
    json: web::Json<GuestRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }

    let subject = json.label.as_deref().unwrap_or("guest");

    let token = match state.auth.issue_guest_token(
        subject,
        vec![
            "stream:read".to_string(),
            "system:read".to_string(),
            "guest:connect".to_string(),
            "phone:connect".to_string(),
        ],
    ) {
        Ok(token) => token,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to issue token"),
    };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "auth.issue_guest_token",
            subject.to_string(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "scopes": ["stream:read", "system:read", "guest:connect", "phone:connect"],
            }),
        ),
    );

    HttpResponse::Ok().json(TokenResponse {
        token,
        role: Role::Guest,
        expires_in_seconds: state.auth.guest_ttl_seconds(),
        refresh_expires_in_seconds: None,
    })
}

#[utoipa::path(
    post,
    path = "/api/auth/session",
    tag = "auth",
    responses(
        (status = 200, description = "Session created", body = TokenResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
#[post("/api/auth/session")]
pub async fn create_session(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Some(token) = extract_bearer_token(&req) {
        let ctx = match state.auth.verify_token(&token) {
            Ok(ctx) => ctx,
            Err(_) => return HttpResponse::Unauthorized().body("Invalid auth token"),
        };
        let session = match state
            .auth
            .issue_session_tokens(&ctx.sub, ctx.role, ctx.scopes.clone())
            .await
        {
            Ok(session) => session,
            Err(_) => return HttpResponse::InternalServerError().body("Failed to issue session"),
        };
        let cookie =
            build_session_cookie(&session.access_token, session.access_ttl.as_secs(), &state);
        let refresh_cookie = build_refresh_cookie(
            &session.refresh_token,
            session.refresh_ttl.as_secs(),
            &state,
        );
        let csrf_cookie =
            build_csrf_cookie(&session.csrf_token, session.refresh_ttl.as_secs(), &state);
        log_audit(
            &req,
            &state,
            AuditEvent::new(
                ctx.sub.clone(),
                format!("{:?}", ctx.role),
                "auth.session.create",
                "bearer",
                "success",
                req.peer_addr().map(|p| p.ip().to_string()),
                serde_json::json!({ "role": format!("{:?}", ctx.role) }),
            ),
        );

        return HttpResponse::Ok()
            .cookie(cookie)
            .cookie(refresh_cookie)
            .cookie(csrf_cookie)
            .json(TokenResponse {
                token: session.access_token,
                role: ctx.role,
                expires_in_seconds: session.access_ttl.as_secs(),
                refresh_expires_in_seconds: Some(session.refresh_ttl.as_secs()),
            });
    }

    if let Some(ctx) = req.extensions().get::<AuthContext>() {
        if ctx.role != Role::Admin {
            return HttpResponse::Forbidden().body("Admin access required");
        }
        let session = match state
            .auth
            .issue_session_tokens("api_key", Role::Admin, vec!["*".to_string()])
            .await
        {
            Ok(session) => session,
            Err(_) => return HttpResponse::InternalServerError().body("Failed to issue session"),
        };
        let cookie =
            build_session_cookie(&session.access_token, session.access_ttl.as_secs(), &state);
        let refresh_cookie = build_refresh_cookie(
            &session.refresh_token,
            session.refresh_ttl.as_secs(),
            &state,
        );
        let csrf_cookie =
            build_csrf_cookie(&session.csrf_token, session.refresh_ttl.as_secs(), &state);
        log_audit(
            &req,
            &state,
            AuditEvent::new(
                "api_key".to_string(),
                "Admin".to_string(),
                "auth.session.create",
                "api_key",
                "success",
                req.peer_addr().map(|p| p.ip().to_string()),
                serde_json::json!({ "role": "Admin" }),
            ),
        );

        return HttpResponse::Ok()
            .cookie(cookie)
            .cookie(refresh_cookie)
            .cookie(csrf_cookie)
            .json(TokenResponse {
                token: session.access_token,
                role: Role::Admin,
                expires_in_seconds: session.access_ttl.as_secs(),
                refresh_expires_in_seconds: Some(session.refresh_ttl.as_secs()),
            });
    }

    HttpResponse::Unauthorized().body("Missing auth token or API key")
}

#[utoipa::path(
    delete,
    path = "/api/auth/session",
    tag = "auth",
    responses(
        (status = 200, description = "Session cleared", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[delete("/api/auth/session")]
pub async fn clear_session(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Some(refresh) = req.cookie(REFRESH_COOKIE_NAME) {
        state.auth.revoke_refresh_token(refresh.value()).await;
    }
    let cookie = Cookie::build(SESSION_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(true)
        .same_site(SameSite::Lax)
        .finish();
    let refresh_cookie = Cookie::build(REFRESH_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(true)
        .same_site(SameSite::Lax)
        .finish();
    let csrf_cookie = Cookie::build(CSRF_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(false)
        .same_site(SameSite::Lax)
        .finish();
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "auth.session.clear",
            "session",
            "success",
            audit_actor(&req).2,
            serde_json::json!({}),
        ),
    );
    HttpResponse::Ok()
        .cookie(cookie)
        .cookie(refresh_cookie)
        .cookie(csrf_cookie)
        .json(serde_json::json!({"status": "cleared"}))
}

#[utoipa::path(
    post,
    path = "/api/auth/refresh",
    tag = "auth",
    responses(
        (status = 200, description = "Session refreshed", body = TokenResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
#[post("/api/auth/refresh")]
pub async fn refresh_session(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if !csrf_header_matches(&req) {
        return HttpResponse::Forbidden().body("CSRF token missing or invalid");
    }
    let refresh = match req.cookie(REFRESH_COOKIE_NAME) {
        Some(cookie) => cookie.value().to_string(),
        None => return HttpResponse::Unauthorized().body("Missing refresh token"),
    };
    let session = match state.auth.refresh_session(&refresh).await {
        Ok(session) => session,
        Err(_) => return HttpResponse::Unauthorized().body("Invalid refresh token"),
    };
    let cookie = build_session_cookie(&session.access_token, session.access_ttl.as_secs(), &state);
    let refresh_cookie = build_refresh_cookie(
        &session.refresh_token,
        session.refresh_ttl.as_secs(),
        &state,
    );
    let csrf_cookie = build_csrf_cookie(&session.csrf_token, session.refresh_ttl.as_secs(), &state);

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            session.subject.clone(),
            format!("{:?}", session.role),
            "auth.session.refresh",
            "refresh_token",
            "success",
            req.peer_addr().map(|p| p.ip().to_string()),
            serde_json::json!({ "role": format!("{:?}", session.role) }),
        ),
    );

    HttpResponse::Ok()
        .cookie(cookie)
        .cookie(refresh_cookie)
        .cookie(csrf_cookie)
        .json(TokenResponse {
            token: session.access_token,
            role: session.role,
            expires_in_seconds: session.access_ttl.as_secs(),
            refresh_expires_in_seconds: Some(session.refresh_ttl.as_secs()),
        })
}

#[utoipa::path(
    post,
    path = "/api/auth/logout_all",
    tag = "auth",
    responses(
        (status = 200, description = "All sessions revoked", body = serde_json::Value)
    )
)]
#[post("/api/auth/logout_all")]
pub async fn logout_all(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (actor, role, ip) = audit_actor(&req);
    state.auth.revoke_all_for_subject(&actor).await;
    let cookie = Cookie::build(SESSION_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(true)
        .same_site(SameSite::Lax)
        .finish();
    let refresh_cookie = Cookie::build(REFRESH_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(true)
        .same_site(SameSite::Lax)
        .finish();
    let csrf_cookie = Cookie::build(CSRF_COOKIE_NAME, "")
        .path("/")
        .max_age(CookieDuration::seconds(0))
        .http_only(false)
        .same_site(SameSite::Lax)
        .finish();

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            actor,
            role,
            "auth.session.logout_all",
            "refresh_tokens",
            "success",
            ip,
            serde_json::json!({}),
        ),
    );

    HttpResponse::Ok()
        .cookie(cookie)
        .cookie(refresh_cookie)
        .cookie(csrf_cookie)
        .json(serde_json::json!({ "status": "revoked" }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PairingStartRequest {
    pub label: Option<String>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PairingStartResponse {
    pub code: String,
    pub expires_in_seconds: u64,
}

#[utoipa::path(
    post,
    path = "/api/auth/pairing/start",
    tag = "auth",
    request_body = PairingStartRequest,
    responses(
        (status = 200, description = "Pairing code issued", body = PairingStartResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
#[post("/api/auth/pairing/start")]
pub async fn pairing_start(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<PairingStartRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let scopes = sanitize_guest_scopes(body.scopes.clone());
    let scopes_for_log = scopes.clone();
    let code = match state
        .auth
        .issue_pairing_code(scopes, body.label.clone())
        .await
    {
        Ok(code) => code,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to issue pairing code"),
    };
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "auth.pairing.start",
            "pairing_code",
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "label": body.label,
                "scopes": scopes_for_log,
            }),
        ),
    );

    HttpResponse::Ok().json(PairingStartResponse {
        code,
        expires_in_seconds: state.auth.pairing_ttl_seconds(),
    })
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PairingClaimRequest {
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BootstrapStatusResponse {
    pub required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BootstrapRequest {
    pub email: String,
    pub name: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApiTokenRequest {
    pub name: String,
    pub scopes: Option<Vec<String>>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenResponse {
    pub token: String,
    pub token_id: String,
    pub name: String,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenSummary {
    pub token_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used: Option<i64>,
    pub revoked: bool,
}

#[utoipa::path(
    get,
    path = "/api/auth/bootstrap/status",
    tag = "auth",
    responses(
        (status = 200, description = "Bootstrap status", body = BootstrapStatusResponse)
    )
)]
#[get("/api/auth/bootstrap/status")]
pub async fn bootstrap_status(state: web::Data<AppState>) -> impl Responder {
    let required = state.auth.bootstrap_required().await.unwrap_or(true);
    HttpResponse::Ok().json(BootstrapStatusResponse { required })
}

#[utoipa::path(
    post,
    path = "/api/auth/bootstrap",
    tag = "auth",
    request_body = BootstrapRequest,
    responses(
        (status = 200, description = "Bootstrap complete", body = TokenResponse),
        (status = 409, description = "Already initialized")
    )
)]
#[post("/api/auth/bootstrap")]
pub async fn bootstrap_admin(
    req: HttpRequest,
    state: web::Data<AppState>,
    json: web::Json<BootstrapRequest>,
) -> impl Responder {
    let required = (state.auth.bootstrap_required().await).unwrap_or(true);
    if !required {
        return HttpResponse::Conflict().body("Bootstrap already completed");
    }
    if json.password.len() < 12 {
        return HttpResponse::BadRequest().body("Password must be at least 12 characters");
    }
    let user = match state
        .auth
        .create_admin_user(&json.email, &json.name, &json.password)
        .await
    {
        Ok(user) => user,
        Err(_) => return HttpResponse::Conflict().body("Admin already exists"),
    };
    let _ = state.auth.mark_bootstrap_complete().await;
    let session = match state
        .auth
        .issue_session_tokens(&user.id, Role::Admin, vec!["*".to_string()])
        .await
    {
        Ok(session) => session,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to issue session"),
    };
    let cookie = build_session_cookie(&session.access_token, session.access_ttl.as_secs(), &state);
    let refresh_cookie = build_refresh_cookie(
        &session.refresh_token,
        session.refresh_ttl.as_secs(),
        &state,
    );
    let csrf_cookie = build_csrf_cookie(&session.csrf_token, session.refresh_ttl.as_secs(), &state);
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            user.email,
            "Admin".to_string(),
            "auth.bootstrap",
            "bootstrap",
            "success",
            req.peer_addr().map(|p| p.ip().to_string()),
            serde_json::json!({}),
        ),
    );
    HttpResponse::Ok()
        .cookie(cookie)
        .cookie(refresh_cookie)
        .cookie(csrf_cookie)
        .json(TokenResponse {
            token: session.access_token,
            role: Role::Admin,
            expires_in_seconds: session.access_ttl.as_secs(),
            refresh_expires_in_seconds: Some(session.refresh_ttl.as_secs()),
        })
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login success", body = TokenResponse),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/auth/login")]
pub async fn login(
    req: HttpRequest,
    state: web::Data<AppState>,
    json: web::Json<LoginRequest>,
) -> impl Responder {
    let user = match state
        .auth
        .verify_password_login(&json.email, &json.password)
        .await
    {
        Ok(user) => user,
        Err(_) => return HttpResponse::Unauthorized().body("Invalid credentials"),
    };
    let role = if user.role == "Admin" {
        Role::Admin
    } else {
        Role::Guest
    };
    let session = match state
        .auth
        .issue_session_tokens(&user.id, role, vec!["*".to_string()])
        .await
    {
        Ok(session) => session,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to issue session"),
    };
    let cookie = build_session_cookie(&session.access_token, session.access_ttl.as_secs(), &state);
    let refresh_cookie = build_refresh_cookie(
        &session.refresh_token,
        session.refresh_ttl.as_secs(),
        &state,
    );
    let csrf_cookie = build_csrf_cookie(&session.csrf_token, session.refresh_ttl.as_secs(), &state);
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            user.email,
            format!("{:?}", role),
            "auth.login",
            "password",
            "success",
            req.peer_addr().map(|p| p.ip().to_string()),
            serde_json::json!({}),
        ),
    );
    HttpResponse::Ok()
        .cookie(cookie)
        .cookie(refresh_cookie)
        .cookie(csrf_cookie)
        .json(TokenResponse {
            token: session.access_token,
            role,
            expires_in_seconds: session.access_ttl.as_secs(),
            refresh_expires_in_seconds: Some(session.refresh_ttl.as_secs()),
        })
}

#[utoipa::path(
    post,
    path = "/api/auth/tokens",
    tag = "auth",
    request_body = ApiTokenRequest,
    responses(
        (status = 200, description = "API token created", body = ApiTokenResponse),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/auth/tokens")]
pub async fn create_api_token(
    req: HttpRequest,
    state: web::Data<AppState>,
    json: web::Json<ApiTokenRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let actor = match req.extensions().get::<AuthContext>() {
        Some(ctx) => ctx.sub.clone(),
        None => return HttpResponse::Unauthorized().body("Unauthorized"),
    };
    if actor == "api_key" {
        return HttpResponse::Forbidden().body("API key cannot mint tokens");
    }
    let scopes = json.scopes.clone().unwrap_or_else(|| vec!["*".to_string()]);
    let expires_at = json.expires_in_seconds.map(|ttl| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now + ttl as i64
    });
    let (raw, token) = match state
        .auth
        .create_api_token(&actor, &json.name, scopes.clone(), expires_at)
        .await
    {
        Ok(out) => out,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to create token"),
    };
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            actor,
            "Admin".to_string(),
            "auth.api_token.create",
            json.name.clone(),
            "success",
            req.peer_addr().map(|p| p.ip().to_string()),
            serde_json::json!({ "token_id": token.id }),
        ),
    );
    HttpResponse::Ok().json(ApiTokenResponse {
        token: raw,
        token_id: token.id,
        name: token.name,
        expires_at: token.expires_at,
        scopes,
    })
}

#[utoipa::path(
    get,
    path = "/api/auth/tokens",
    tag = "auth",
    responses(
        (status = 200, description = "API tokens", body = [ApiTokenSummary]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/auth/tokens")]
pub async fn list_api_tokens(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let actor = match req.extensions().get::<AuthContext>() {
        Some(ctx) => ctx.sub.clone(),
        None => return HttpResponse::Unauthorized().body("Unauthorized"),
    };
    let tokens = match state.auth.list_api_tokens(&actor).await {
        Ok(tokens) => tokens,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to list tokens"),
    };
    let summaries: Vec<ApiTokenSummary> = tokens
        .into_iter()
        .map(|token| ApiTokenSummary {
            token_id: token.id,
            name: token.name,
            scopes: token.scopes,
            created_at: token.created_at,
            expires_at: token.expires_at,
            last_used: token.last_used,
            revoked: token.revoked,
        })
        .collect();
    HttpResponse::Ok().json(summaries)
}

#[utoipa::path(
    delete,
    path = "/api/auth/tokens/{token_id}",
    tag = "auth",
    params(("token_id" = String, Path, description = "Token id")),
    responses(
        (status = 200, description = "Token revoked", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[delete("/api/auth/tokens/{token_id}")]
pub async fn revoke_api_token(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let token_id = path.into_inner();
    let _ = state.auth.revoke_api_token(&token_id).await;
    HttpResponse::Ok().json(serde_json::json!({ "status": "revoked", "token_id": token_id }))
}
#[utoipa::path(
    post,
    path = "/api/auth/pairing/claim",
    tag = "auth",
    request_body = PairingClaimRequest,
    responses(
        (status = 200, description = "Pairing claimed", body = TokenResponse),
        (status = 400, description = "Invalid pairing code"),
        (status = 429, description = "Rate limited")
    )
)]
#[post("/api/auth/pairing/claim")]
pub async fn pairing_claim(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<PairingClaimRequest>,
) -> impl Responder {
    let ip = req.peer_addr().map(|p| p.ip());
    let grant = match state.auth.consume_pairing_code(&body.code, ip).await {
        Ok(grant) => grant,
        Err(crate::auth::AuthError::RateLimited) => {
            return HttpResponse::TooManyRequests().body("Pairing rate limited");
        }
        Err(_) => return HttpResponse::BadRequest().body("Invalid pairing code"),
    };

    let subject = grant
        .label
        .as_ref()
        .map(|label| format!("guest:{}", label))
        .unwrap_or_else(|| "guest".to_string());

    let role = grant.role;
    let token = match role {
        Role::Admin => state.auth.issue_admin_token(&subject, grant.scopes),
        Role::Guest => state.auth.issue_guest_token(&subject, grant.scopes),
    };
    let token = match token {
        Ok(token) => token,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to issue token"),
    };

    let expires_in_seconds = match role {
        Role::Admin => state.auth.admin_ttl_seconds(),
        Role::Guest => state.auth.guest_ttl_seconds(),
    };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            subject.clone(),
            format!("{:?}", role),
            "auth.pairing.claim",
            "pairing_code",
            "success",
            req.peer_addr().map(|p| p.ip().to_string()),
            serde_json::json!({ "role": format!("{:?}", role) }),
        ),
    );

    HttpResponse::Ok().json(TokenResponse {
        token,
        role,
        expires_in_seconds,
        refresh_expires_in_seconds: None,
    })
}

fn sanitize_guest_scopes(requested: Option<Vec<String>>) -> Vec<String> {
    let allowed = [
        "stream:read",
        "system:read",
        "guest:connect",
        "phone:connect",
    ];
    if let Some(scopes) = requested {
        let mut filtered: Vec<String> = scopes
            .into_iter()
            .filter(|s| allowed.iter().any(|a| a == s))
            .collect();
        if filtered.is_empty() {
            filtered = allowed.iter().map(|s| s.to_string()).collect();
        }
        filtered
    } else {
        allowed.iter().map(|s| s.to_string()).collect()
    }
}

fn extract_bearer_token(req: &HttpRequest) -> Option<String> {
    let header = req.headers().get(actix_web::http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_string())
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let (actor, role) = match req.extensions().get::<AuthContext>() {
        Some(ctx) => (ctx.sub.clone(), format!("{:?}", ctx.role)),
        None => ("unknown".to_string(), "unknown".to_string()),
    };
    let ip = req.peer_addr().map(|p| p.ip().to_string());
    (actor, role, ip)
}

fn log_audit(req: &HttpRequest, state: &web::Data<AppState>, event: AuditEvent) {
    if let Err(err) = state
        .audit
        .append_with_redaction(event, &state.config.privacy)
    {
        tracing::warn!("audit log failed for {}: {}", req.path(), err);
    }
}

fn build_session_cookie(token: &str, ttl_seconds: u64, state: &AppState) -> Cookie<'static> {
    let secure = state.config.server.cookie_secure.unwrap_or(false);
    let mut builder = Cookie::build(SESSION_COOKIE_NAME, token.to_string())
        .path("/")
        .max_age(CookieDuration::seconds(ttl_seconds as i64))
        .http_only(true)
        .same_site(SameSite::Lax);
    if secure {
        builder = builder.secure(true);
    }
    builder.finish()
}

fn build_refresh_cookie(token: &str, ttl_seconds: u64, state: &AppState) -> Cookie<'static> {
    let secure = state.config.server.cookie_secure.unwrap_or(false);
    let mut builder = Cookie::build(REFRESH_COOKIE_NAME, token.to_string())
        .path("/")
        .max_age(CookieDuration::seconds(ttl_seconds as i64))
        .http_only(true)
        .same_site(SameSite::Lax);
    if secure {
        builder = builder.secure(true);
    }
    builder.finish()
}

fn build_csrf_cookie(token: &str, ttl_seconds: u64, state: &AppState) -> Cookie<'static> {
    let secure = state.config.server.cookie_secure.unwrap_or(false);
    let mut builder = Cookie::build(CSRF_COOKIE_NAME, token.to_string())
        .path("/")
        .max_age(CookieDuration::seconds(ttl_seconds as i64))
        .http_only(false)
        .same_site(SameSite::Lax);
    if secure {
        builder = builder.secure(true);
    }
    builder.finish()
}

fn csrf_header_matches(req: &HttpRequest) -> bool {
    let cookie = match req.cookie(CSRF_COOKIE_NAME) {
        Some(cookie) => cookie.value().to_string(),
        None => return false,
    };
    let header = match req.headers().get(CSRF_HEADER_NAME) {
        Some(header) => header,
        None => return false,
    };
    let header_value = match header.to_str() {
        Ok(value) => value,
        Err(_) => return false,
    };
    cookie == header_value
}
