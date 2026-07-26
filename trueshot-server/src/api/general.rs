use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::state::AppState;
use actix_web::{get, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use std::io::{Read, Seek, SeekFrom};

/// Health check endpoint
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "general",
    responses(
        (status = 200, description = "Service healthy", body = serde_json::Value)
    )
)]
#[get("/api/health")]
pub async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok", "version": "6.8.0" }))
}

/// Get recent log entries
#[utoipa::path(
    get,
    path = "/api/logs",
    tag = "general",
    responses(
        (status = 200, description = "Recent log lines", body = [String]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/logs")]
pub async fn get_logs(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    match tokio::task::spawn_blocking(|| read_log_tail("logs/trueshot.log", 512 * 1024)).await {
        Ok(Ok(content)) => {
            let redacted = redact_log_content(&content);
            let lines: Vec<&str> = redacted.lines().rev().take(100).collect();
            HttpResponse::Ok().json(lines)
        }
        Ok(Err(e)) => HttpResponse::InternalServerError().body(e.to_string()),
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/api/logs/export",
    tag = "general",
    responses(
        (status = 200, description = "Log export", body = String),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/logs/export")]
pub async fn export_logs(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    match tokio::task::spawn_blocking(|| read_log_tail("logs/trueshot.log", 5 * 1024 * 1024)).await
    {
        Ok(Ok(content)) => {
            let redacted = redact_log_content(&content);
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    audit_actor(&req).0,
                    audit_actor(&req).1,
                    "logs.export",
                    "trueshot.log",
                    "success",
                    audit_actor(&req).2,
                    serde_json::json!({ "bytes": redacted.len() }),
                ),
            );
            HttpResponse::Ok()
                .content_type("text/plain")
                .append_header((
                    "Content-Disposition",
                    "attachment; filename=\"trueshot_logs.txt\"",
                ))
                .body(redacted)
        }
        Ok(Err(e)) => HttpResponse::InternalServerError().body(e.to_string()),
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let (actor, role) = match req.extensions().get::<crate::auth::AuthContext>() {
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

fn read_log_tail(path: &str, max_bytes: usize) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = if len as usize > max_bytes {
        len - max_bytes as u64
    } else {
        0
    };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn redact_log_content(content: &str) -> String {
    let mut redacted = content.to_string();
    for marker in [
        "Bearer ",
        "access_token=",
        "refresh_token=",
        "api_key=",
        "token=",
        "password=",
        "secret=",
    ] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redacted
}

fn redact_after_marker(input: &str, marker: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while let Some(pos) = input[idx..].find(marker) {
        let start = idx + pos;
        let token_start = start + marker.len();
        out.push_str(&input[idx..token_start]);

        let mut end = token_start;
        let bytes = input.as_bytes();
        while end < input.len() {
            let ch = bytes[end] as char;
            if ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '&' || ch == ';' || ch == ','
            {
                break;
            }
            end += 1;
        }

        out.push_str("[redacted]");
        idx = end;
    }
    out.push_str(&input[idx..]);
    out
}
