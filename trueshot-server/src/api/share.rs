use actix_files::NamedFile;
use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::annotations::load_annotations_for_asset;
use crate::at_rest::{decrypt_file_in_place, require_master_key, ProjectKeyStore};
use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::config::AppConfig;
use crate::fs_safety::resolve_project_child_file;
use crate::licensing::require_license_feature;
use crate::state::AppState;
use nalgebra as na;
use std::io::{BufRead, BufReader, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::task;
use tokio_util::io::ReaderStream;
use trueshot_core::export::gltf::export_glb;
use trueshot_core::export::obj::export_obj;
use trueshot_core::export::ply::{export_ply, PlyExportOptions};
use trueshot_core::licensing::Feature;
use trueshot_core::mesh::{apply_mesh_edits, load_mesh, MeshEditOp};
use trueshot_core::reconstruction::{Face, Mesh};

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateShareRequest {
    pub project_id: String,
    pub asset_path: String,
    pub expires_in_seconds: Option<u64>,
    pub max_uses: Option<u64>,
    pub allow_download: Option<bool>,
    pub allow_embed: Option<bool>,
    pub public: Option<bool>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub short_code: Option<String>,
    pub cover_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShareAssetQuery {
    pub download: Option<bool>,
    pub lod: Option<u8>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareLinkResponse {
    pub token: String,
    pub asset_url: String,
    pub download_url: String,
    pub viewer_url: String,
    pub short_url: Option<String>,
    pub card_url: String,
    pub lods: Vec<ShareAssetLod>,
    pub expires_at: i64,
    pub max_uses: Option<i64>,
    pub remaining_uses: Option<i64>,
    pub allow_download: bool,
    pub allow_embed: bool,
    pub project_id: String,
    pub asset_path: String,
    pub public: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareLinkMetadata {
    pub asset_url: String,
    pub download_url: String,
    pub viewer_url: String,
    pub short_url: Option<String>,
    pub card_url: String,
    pub lods: Vec<ShareAssetLod>,
    pub expires_at: i64,
    pub max_uses: Option<i64>,
    pub remaining_uses: Option<i64>,
    pub allow_download: bool,
    pub allow_embed: bool,
    pub project_id: String,
    pub asset_path: String,
    pub public: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareReferrerEntry {
    pub referrer: String,
    pub count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareAssetLod {
    pub level: u8,
    pub asset_url: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShareAnalyticsResponse {
    pub views: i64,
    pub asset_requests: i64,
    pub downloads: i64,
    pub embeds: i64,
    pub last_access: Option<i64>,
    pub top_referrers: Vec<ShareReferrerEntry>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SharePublicRequest {
    pub public: bool,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub short_code: Option<String>,
    pub cover_path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SharePublicResponse {
    pub public: bool,
    pub short_url: Option<String>,
    pub card_url: String,
    pub viewer_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicShareSummary {
    pub token: String,
    pub short_code: String,
    pub short_url: String,
    pub viewer_url: String,
    pub card_url: String,
    pub asset_url: String,
    pub download_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub views: i64,
}

#[derive(Debug, Deserialize)]
pub struct PublicShareQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub tag: Option<String>,
    pub sort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShareAnnotationQuery {
    pub layer: Option<String>,
    pub asset_path: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/share",
    tag = "share",
    request_body = CreateShareRequest,
    responses(
        (status = 200, description = "Share link created", body = ShareLinkResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/share")]
pub async fn create_share_link(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<CreateShareRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::CommercialUse, "commercial_use") {
        return resp;
    }

    let project_id = payload.project_id.trim();
    let asset_path = payload.asset_path.trim();
    if project_id.is_empty() || asset_path.is_empty() {
        return HttpResponse::BadRequest().body("Missing project_id or asset_path");
    }

    let resolved = match resolve_share_asset(&state.config, project_id, asset_path, None) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    if !resolved.exists() {
        return HttpResponse::NotFound().body("Asset not found");
    }

    if should_generate_lods() {
        let state = state.clone();
        let project_id = project_id.to_string();
        let resolved = resolved.clone();
        task::spawn_blocking(move || {
            if let Err(err) = ensure_share_lods(&state, &project_id, &resolved) {
                tracing::warn!("Failed to generate share LODs: {}", err);
            }
        });
    }

    let ttl_seconds = payload.expires_in_seconds.unwrap_or(60 * 60 * 24 * 7);
    let allow_download = payload.allow_download.unwrap_or(true);
    let allow_embed = payload.allow_embed.unwrap_or(true);

    let (token, link) = match state
        .auth
        .create_share_link(
            project_id,
            asset_path,
            ttl_seconds,
            payload.max_uses,
            allow_download,
            allow_embed,
        )
        .await
    {
        Ok(result) => result,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    let (asset_url, download_url, viewer_url) = share_urls(&req, &state.config, &token);
    let card_url = share_card_url(&req, &state.config, &token);
    let lods = share_lods(&req, &state.config, &token, project_id, asset_path);
    let remaining = link.max_uses.map(|max| max.saturating_sub(link.uses));
    let mut short_url = None;
    let mut public_flag = None;

    if payload.public.unwrap_or(false) {
        match state
            .auth
            .upsert_share_public(
                &token,
                true,
                payload.title.clone(),
                payload.description.clone(),
                payload.tags.clone().unwrap_or_default(),
                payload.cover_path.clone(),
                payload.short_code.clone(),
            )
            .await
        {
            Ok(public_entry) => {
                public_flag = Some(true);
                short_url = Some(share_short_url(
                    &req,
                    &state.config,
                    &public_entry.short_code,
                ));
            }
            Err(err) => {
                tracing::warn!("Failed to publish share: {}", err);
            }
        }
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "share.create",
            project_id.to_string(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "asset_path": asset_path,
                "expires_at": link.expires_at,
                "max_uses": link.max_uses,
                "allow_download": allow_download,
                "allow_embed": allow_embed
            }),
        ),
    );

    HttpResponse::Ok().json(ShareLinkResponse {
        token,
        asset_url,
        download_url,
        viewer_url,
        short_url,
        card_url,
        lods,
        expires_at: link.expires_at,
        max_uses: link.max_uses,
        remaining_uses: remaining,
        allow_download,
        allow_embed,
        project_id: project_id.to_string(),
        asset_path: asset_path.to_string(),
        public: public_flag,
    })
}

#[utoipa::path(
    get,
    path = "/api/share/{token}",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    responses(
        (status = 200, description = "Share link metadata", body = ShareLinkMetadata),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/share/{token}")]
pub async fn get_share_link(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let token = path.into_inner();
    let link = match state.auth.get_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    let (asset_url, download_url, viewer_url) = share_urls(&req, &state.config, &token);
    let card_url = share_card_url(&req, &state.config, &token);
    let lods = share_lods(
        &req,
        &state.config,
        &token,
        &link.project_id,
        &link.asset_path,
    );
    let remaining = link.max_uses.map(|max| max.saturating_sub(link.uses));
    let mut short_url = None;
    let mut public_flag = None;
    if let Ok(Some(public_entry)) = state.auth.get_share_public(&token).await {
        if public_entry.is_public {
            public_flag = Some(true);
            short_url = Some(share_short_url(
                &req,
                &state.config,
                &public_entry.short_code,
            ));
        }
    }
    log_share_access(&state, &token, &req, "meta", false, false).await;

    HttpResponse::Ok().json(ShareLinkMetadata {
        asset_url,
        download_url,
        viewer_url,
        short_url,
        card_url,
        lods,
        expires_at: link.expires_at,
        max_uses: link.max_uses,
        remaining_uses: remaining,
        allow_download: link.allow_download,
        allow_embed: link.allow_embed,
        project_id: link.project_id,
        asset_path: link.asset_path,
        public: public_flag,
    })
}

#[utoipa::path(
    get,
    path = "/api/share/{token}/asset",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    responses(
        (status = 200, description = "Shared asset"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/share/{token}/asset")]
pub async fn get_share_asset(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<ShareAssetQuery>,
) -> impl Responder {
    let token = path.into_inner();
    let link = match state.auth.consume_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    let download = query.download.unwrap_or(false);
    let lod = query.lod;
    if download && !link.allow_download {
        return HttpResponse::Forbidden().body("Downloads disabled");
    }
    if !download && !link.allow_embed {
        return HttpResponse::Forbidden().body("Embeds disabled");
    }

    let resolved = match resolve_share_asset(&state.config, &link.project_id, &link.asset_path, lod)
    {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.filter(|v| *v > 0);
    if offset > 0 || limit.is_some() {
        match open_share_file_path(&state, &link.project_id, &resolved).await {
            Ok(path) => {
                log_share_access(&state, &token, &req, "asset", !download, download).await;
                stream_share_range(&path, offset, limit, download).await
            }
            Err(resp) => resp,
        }
    } else if let Some(range_header) = req.headers().get(actix_web::http::header::RANGE) {
        match open_share_file_path(&state, &link.project_id, &resolved).await {
            Ok(path) => {
                let file_size = match std::fs::metadata(&path) {
                    Ok(meta) => meta.len(),
                    Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
                };
                let header_str = match range_header.to_str() {
                    Ok(value) => value,
                    Err(_) => return HttpResponse::BadRequest().body("Invalid Range header"),
                };
                match parse_range_header(header_str, file_size) {
                    Ok((start, end)) => {
                        log_share_access(&state, &token, &req, "asset", !download, download).await;
                        let limit = end.saturating_sub(start).saturating_add(1);
                        stream_share_range(&path, start, Some(limit), download).await
                    }
                    Err(()) => HttpResponse::RangeNotSatisfiable()
                        .append_header(("Content-Range", format!("bytes */{}", file_size)))
                        .finish(),
                }
            }
            Err(resp) => resp,
        }
    } else {
        match open_share_file(&state, &link.project_id, &resolved).await {
            Ok(mut file) => {
                log_share_access(&state, &token, &req, "asset", !download, download).await;
                if download {
                    file =
                        file.set_content_disposition(actix_web::http::header::ContentDisposition {
                            disposition: actix_web::http::header::DispositionType::Attachment,
                            parameters: vec![],
                        });
                }
                file.into_response(&req)
            }
            Err(resp) => resp,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/share/{token}/analytics",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    responses(
        (status = 200, description = "Share analytics", body = ShareAnalyticsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/share/{token}/analytics")]
pub async fn get_share_analytics(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::TeamCollaboration, "team_collaboration")
    {
        return resp;
    }
    let token = path.into_inner();
    let link = match state.auth.get_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let analytics = match state.auth.get_share_analytics(&token).await {
        Ok(data) => data,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let top_referrers = analytics
        .top_referrers
        .into_iter()
        .map(|entry| ShareReferrerEntry {
            referrer: entry.referrer,
            count: entry.count,
        })
        .collect();
    HttpResponse::Ok().json(ShareAnalyticsResponse {
        views: analytics.views,
        asset_requests: analytics.asset_requests,
        downloads: analytics.downloads,
        embeds: analytics.embeds,
        last_access: analytics.last_access.or(link.last_access),
        top_referrers,
    })
}

#[utoipa::path(
    post,
    path = "/api/share/{token}/public",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    request_body = SharePublicRequest,
    responses(
        (status = 200, description = "Share public metadata", body = SharePublicResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/share/{token}/public")]
pub async fn set_share_public(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
    payload: web::Json<SharePublicRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::TeamCollaboration, "team_collaboration")
    {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::CommercialUse, "commercial_use") {
        return resp;
    }
    let token = path.into_inner();
    let link = match state.auth.get_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let entry = match state
        .auth
        .upsert_share_public(
            &token,
            payload.public,
            payload.title.clone(),
            payload.description.clone(),
            payload.tags.clone().unwrap_or_default(),
            payload.cover_path.clone(),
            payload.short_code.clone(),
        )
        .await
    {
        Ok(entry) => entry,
        Err(err) => return HttpResponse::BadRequest().body(err.to_string()),
    };
    let (_, _, viewer_url) = share_urls(&req, &state.config, &token);
    let short_url = if entry.is_public {
        Some(share_short_url(&req, &state.config, &entry.short_code))
    } else {
        None
    };
    let card_url = share_card_url(&req, &state.config, &token);
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "share.publish",
            link.project_id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "public": entry.is_public,
                "short_code": entry.short_code,
                "title": entry.title,
                "tags": entry.tags
            }),
        ),
    );
    HttpResponse::Ok().json(SharePublicResponse {
        public: entry.is_public,
        short_url,
        card_url,
        viewer_url,
        title: entry.title,
        description: entry.description,
        tags: entry.tags,
        cover_path: entry.cover_path,
        created_at: entry.created_at,
        updated_at: entry.updated_at,
    })
}

#[utoipa::path(
    get,
    path = "/api/share/{token}/public",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    responses(
        (status = 200, description = "Share public metadata", body = SharePublicResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/share/{token}/public")]
pub async fn get_share_public(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::TeamCollaboration, "team_collaboration")
    {
        return resp;
    }
    let token = path.into_inner();
    let entry = match state.auth.get_share_public(&token).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return HttpResponse::NotFound().body("Public share not configured"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let (_, _, viewer_url) = share_urls(&req, &state.config, &token);
    let short_url = if entry.is_public {
        Some(share_short_url(&req, &state.config, &entry.short_code))
    } else {
        None
    };
    let card_url = share_card_url(&req, &state.config, &token);
    HttpResponse::Ok().json(SharePublicResponse {
        public: entry.is_public,
        short_url,
        card_url,
        viewer_url,
        title: entry.title,
        description: entry.description,
        tags: entry.tags,
        cover_path: entry.cover_path,
        created_at: entry.created_at,
        updated_at: entry.updated_at,
    })
}

#[utoipa::path(
    get,
    path = "/api/public/shares",
    tag = "share",
    params(
        ("limit" = Option<i64>, Query, description = "Max items"),
        ("offset" = Option<i64>, Query, description = "Offset for pagination"),
        ("tag" = Option<String>, Query, description = "Filter by tag"),
        ("sort" = Option<String>, Query, description = "Sort by recent or popular")
    ),
    responses(
        (status = 200, description = "Public shares", body = [PublicShareSummary])
    )
)]
#[get("/api/public/shares")]
pub async fn list_public_shares(
    req: HttpRequest,
    state: web::Data<AppState>,
    query: web::Query<PublicShareQuery>,
) -> impl Responder {
    if let Err(resp) =
        require_license_feature(&state, Feature::TeamCollaboration, "team_collaboration")
    {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::CommercialUse, "commercial_use") {
        return resp;
    }
    let limit = query.limit.unwrap_or(24).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);
    let records = match state
        .auth
        .list_public_shares(limit, offset, query.tag.as_deref(), query.sort.as_deref())
        .await
    {
        Ok(records) => records,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let public_base = state
        .config
        .server
        .public_base_url
        .clone()
        .unwrap_or_else(|| {
            format!(
                "{}://{}",
                req.connection_info().scheme(),
                req.connection_info().host()
            )
        });
    let frontend_base = state
        .config
        .server
        .frontend_base_url
        .clone()
        .unwrap_or_else(|| public_base.clone());
    let items = records
        .into_iter()
        .map(|record| {
            let token = record.public_token;
            let asset_url = format!("{public_base}/api/share/{token}/asset");
            let download_url = format!("{public_base}/api/share/{token}/asset?download=true");
            let viewer_url = format!("{frontend_base}/share/{token}");
            let short_url = format!("{public_base}/s/{}", record.short_code);
            let card_url = format!("{public_base}/share/{token}/card");
            PublicShareSummary {
                token,
                short_code: record.short_code,
                short_url,
                viewer_url,
                card_url,
                asset_url,
                download_url,
                title: record.title,
                description: record.description,
                tags: record.tags,
                cover_path: record.cover_path,
                created_at: record.created_at,
                updated_at: record.updated_at,
                views: record.views,
            }
        })
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(items)
}

#[get("/s/{code}")]
pub async fn redirect_short_link(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let code = path.into_inner();
    let entry = match state.auth.get_share_public_by_code(&code).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return HttpResponse::NotFound().body("Short link not found"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    if !entry.is_public {
        return HttpResponse::NotFound().body("Short link not available");
    }
    let (_, _, viewer_url) = share_urls(&req, &state.config, &entry.public_token);
    log_share_access(&state, &entry.public_token, &req, "short", true, false).await;
    HttpResponse::Found()
        .append_header(("Location", viewer_url))
        .finish()
}

#[get("/share/{token}/card")]
pub async fn share_card(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let token = path.into_inner();
    let link = match state.auth.get_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let public_base = state
        .config
        .server
        .public_base_url
        .clone()
        .unwrap_or_else(|| {
            format!(
                "{}://{}",
                req.connection_info().scheme(),
                req.connection_info().host()
            )
        });
    let frontend_base = state
        .config
        .server
        .frontend_base_url
        .clone()
        .unwrap_or_else(|| public_base.clone());
    let viewer_url = format!("{frontend_base}/share/{token}");
    let card_image = format!("{public_base}/assets/share-card.svg");
    let mut title = format!("TrueShot Share • {}", link.asset_path);
    let mut description = "Shared 3D asset".to_string();
    if let Ok(Some(public_entry)) = state.auth.get_share_public(&token).await {
        if let Some(t) = public_entry.title {
            title = t;
        }
        if let Some(d) = public_entry.description {
            description = d;
        }
    }
    let safe_title = escape_html(&title);
    let safe_description = escape_html(&description);
    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{safe_title}</title>
  <meta property="og:title" content="{safe_title}">
  <meta property="og:description" content="{safe_description}">
  <meta property="og:type" content="website">
  <meta property="og:url" content="{viewer_url}">
  <meta property="og:image" content="{card_image}">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="{safe_title}">
  <meta name="twitter:description" content="{safe_description}">
  <meta name="twitter:image" content="{card_image}">
  <meta http-equiv="refresh" content="0; url={viewer_url}">
  <style>
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      background: #0b0d12;
      color: #e6e9ef;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
      margin: 0;
    }}
    .card {{
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 16px;
      padding: 24px 32px;
      background: rgba(255,255,255,0.04);
    }}
    a {{
      color: #7dd3fc;
      text-decoration: none;
    }}
  </style>
</head>
<body>
    <div class="card">
    <div style="font-size: 18px; font-weight: 600; margin-bottom: 8px;">{safe_title}</div>
    <div style="font-size: 14px; opacity: 0.7; margin-bottom: 16px;">{safe_description}</div>
    <a href="{viewer_url}">Open share</a>
  </div>
</body>
</html>"#
    );
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

#[utoipa::path(
    get,
    path = "/api/share/{token}/annotations",
    tag = "share",
    params(("token" = String, Path, description = "Share token")),
    responses(
        (status = 200, description = "Share annotations", body = AnnotationLayer),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/share/{token}/annotations")]
pub async fn get_share_annotations(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<ShareAnnotationQuery>,
) -> impl Responder {
    let token = path.into_inner();
    let link = match state.auth.get_share_link(&token).await {
        Ok(Some(link)) => link,
        Ok(None) => return HttpResponse::NotFound().body("Share link expired or invalid"),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    if !link.allow_embed {
        return HttpResponse::Forbidden().body("Embeds disabled");
    }
    let layer = query.layer.clone().unwrap_or_else(|| "default".to_string());
    let asset_path = query
        .asset_path
        .clone()
        .unwrap_or_else(|| link.asset_path.clone());
    if asset_path != link.asset_path {
        return HttpResponse::Forbidden().body("Asset mismatch");
    }
    let annotations =
        match load_annotations_for_asset(&state, &link.project_id, &asset_path, &layer) {
            Ok(layer) => layer,
            Err(resp) => return resp,
        };
    log_share_access(&state, &token, &req, "annotations", true, false).await;
    HttpResponse::Ok().json(annotations)
}

fn resolve_share_asset(
    config: &AppConfig,
    project_id: &str,
    asset_path: &str,
    lod: Option<u8>,
) -> Result<std::path::PathBuf, HttpResponse> {
    let normalized = asset_path.trim_start_matches('/');
    let (root, rest) = if let Some(rest) = normalized.strip_prefix("output/") {
        ("output", rest)
    } else if let Some(rest) = normalized.strip_prefix("processed/") {
        ("processed", rest)
    } else {
        return Err(
            HttpResponse::BadRequest().body("asset_path must begin with output/ or processed/")
        );
    };
    let rest_path = std::path::Path::new(rest);
    let parent = rest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let file_name = rest_path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    if lod.is_none() {
        return resolve_project_child_file(&config.paths.projects_dir, project_id, root, rest);
    }
    let lod_level = lod.unwrap_or(0);
    let stem = rest_path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or(file_name);
    let ext = rest_path.extension().and_then(|v| v.to_str()).unwrap_or("");
    let mut candidates = Vec::new();
    if ext.is_empty() {
        candidates.push(format!("{stem}.lod{lod_level}"));
        candidates.push(format!("{stem}_lod{lod_level}"));
    } else {
        candidates.push(format!("{stem}.lod{lod_level}.{ext}"));
        candidates.push(format!("{stem}_lod{lod_level}.{ext}"));
    }
    for candidate in candidates {
        let rel = if parent.as_os_str().is_empty() {
            candidate.clone()
        } else {
            parent.join(candidate).to_string_lossy().to_string()
        };
        let resolved =
            resolve_project_child_file(&config.paths.projects_dir, project_id, root, &rel)?;
        if resolved.exists() {
            return Ok(resolved);
        }
        let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            return Ok(resolved);
        }
    }
    Err(HttpResponse::NotFound().body("LOD asset not found"))
}

fn share_urls(req: &HttpRequest, config: &AppConfig, token: &str) -> (String, String, String) {
    let public_base = config.server.public_base_url.clone().unwrap_or_else(|| {
        format!(
            "{}://{}",
            req.connection_info().scheme(),
            req.connection_info().host()
        )
    });
    let frontend_base = config
        .server
        .frontend_base_url
        .clone()
        .unwrap_or_else(|| public_base.clone());
    let asset_url = format!("{public_base}/api/share/{token}/asset");
    let download_url = format!("{public_base}/api/share/{token}/asset?download=true");
    let viewer_url = format!("{frontend_base}/share/{token}");
    (asset_url, download_url, viewer_url)
}

fn share_card_url(req: &HttpRequest, config: &AppConfig, token: &str) -> String {
    let public_base = config.server.public_base_url.clone().unwrap_or_else(|| {
        format!(
            "{}://{}",
            req.connection_info().scheme(),
            req.connection_info().host()
        )
    });
    format!("{public_base}/share/{token}/card")
}

fn share_short_url(req: &HttpRequest, config: &AppConfig, code: &str) -> String {
    let public_base = config.server.public_base_url.clone().unwrap_or_else(|| {
        format!(
            "{}://{}",
            req.connection_info().scheme(),
            req.connection_info().host()
        )
    });
    format!("{public_base}/s/{code}")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn share_lods(
    req: &HttpRequest,
    config: &AppConfig,
    token: &str,
    project_id: &str,
    asset_path: &str,
) -> Vec<ShareAssetLod> {
    let normalized = asset_path.trim_start_matches('/');
    let (root, rest) = if let Some(rest) = normalized.strip_prefix("output/") {
        ("output", rest)
    } else if let Some(rest) = normalized.strip_prefix("processed/") {
        ("processed", rest)
    } else {
        return Vec::new();
    };
    let rest_path = std::path::Path::new(rest);
    let parent = rest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let stem = rest_path.file_stem().and_then(|v| v.to_str()).unwrap_or("");
    let ext = rest_path.extension().and_then(|v| v.to_str()).unwrap_or("");
    if stem.is_empty() {
        return Vec::new();
    }
    let public_base = config.server.public_base_url.clone().unwrap_or_else(|| {
        format!(
            "{}://{}",
            req.connection_info().scheme(),
            req.connection_info().host()
        )
    });
    let mut lods = Vec::new();
    for level in 0u8..5u8 {
        let mut candidates = Vec::new();
        if ext.is_empty() {
            candidates.push(format!("{stem}.lod{level}"));
            candidates.push(format!("{stem}_lod{level}"));
        } else {
            candidates.push(format!("{stem}.lod{level}.{ext}"));
            candidates.push(format!("{stem}_lod{level}.{ext}"));
        }
        for candidate in candidates {
            let rel = if parent.as_os_str().is_empty() {
                candidate.clone()
            } else {
                parent.join(candidate).to_string_lossy().to_string()
            };
            let resolved = match resolve_project_child_file(
                &config.paths.projects_dir,
                project_id,
                root,
                &rel,
            ) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let mut bytes = 0u64;
            let mut exists = resolved.exists();
            if !exists {
                let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
                if enc_path.exists() {
                    exists = true;
                    bytes = std::fs::metadata(&enc_path).map(|m| m.len()).unwrap_or(0);
                }
            } else {
                bytes = std::fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0);
            }
            if exists {
                let asset_url = format!("{public_base}/api/share/{token}/asset?lod={level}");
                lods.push(ShareAssetLod {
                    level,
                    asset_url,
                    bytes,
                });
                break;
            }
        }
    }
    lods.sort_by_key(|lod| lod.level);
    lods
}

fn should_generate_lods() -> bool {
    std::env::var("TRUESHOT_SHARE_LOD_AUTO")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
        .unwrap_or(true)
}

fn parse_lod_ratios() -> Vec<f32> {
    let raw =
        std::env::var("TRUESHOT_SHARE_LOD_RATIOS").unwrap_or_else(|_| "0.35,0.15,0.05".to_string());
    let mut ratios: Vec<f32> = raw
        .split(',')
        .filter_map(|v| v.trim().parse::<f32>().ok())
        .filter(|v| *v > 0.0 && *v < 1.0)
        .collect();
    if ratios.is_empty() {
        ratios = vec![0.35, 0.15, 0.05];
    }
    ratios
}

fn ensure_share_lods(
    state: &web::Data<AppState>,
    project_id: &str,
    base_path: &std::path::Path,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut resolved = base_path.to_path_buf();
    if !resolved.exists() {
        let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            let key_store =
                project_key_store(state).map_err(|e| anyhow!("Key store error: {:?}", e))?;
            let key = key_store.load_or_create(project_id)?;
            if let Ok(restored) = decrypt_file_in_place(&enc_path, &key) {
                resolved = restored;
            }
        }
    }
    let ext = resolved
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !resolved.exists() {
        return Ok(Vec::new());
    }
    let ratios = parse_lod_ratios();
    let mut generated = Vec::new();
    if ext == "obj" {
        let mesh = load_mesh(&resolved)?;
        let base_faces = mesh.faces.len().max(1);
        for (idx, ratio) in ratios.iter().enumerate() {
            let level = (idx + 1) as u8;
            let target = ((base_faces as f32) * ratio).round() as usize;
            if target >= base_faces || target < 4 {
                continue;
            }
            let lod_path = resolved.with_file_name(format!(
                "{}.lod{}.{}",
                resolved
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("lod"),
                level,
                ext
            ));
            if lod_path.exists() {
                generated.push(lod_path);
                continue;
            }
            let mut lod_mesh = mesh.clone();
            let ops = vec![
                MeshEditOp::Decimate {
                    target_triangles: target,
                    preserve_boundaries: true,
                    preserve_uv_seams: true,
                    uv_seam_threshold: 0.2,
                },
                MeshEditOp::RecomputeNormals,
            ];
            apply_mesh_edits(&mut lod_mesh, &ops)?;
            export_obj(&lod_mesh, &lod_path)?;
            generated.push(lod_path);
        }
    } else if ext == "ply" {
        if let Ok(mesh) = load_ply_ascii_mesh(&resolved) {
            if !mesh.faces.is_empty() {
                let base_faces = mesh.faces.len().max(1);
                for (idx, ratio) in ratios.iter().enumerate() {
                    let level = (idx + 1) as u8;
                    let target = ((base_faces as f32) * ratio).round() as usize;
                    if target >= base_faces || target < 4 {
                        continue;
                    }
                    let lod_path = resolved.with_file_name(format!(
                        "{}.lod{}.{}",
                        resolved
                            .file_stem()
                            .and_then(|v| v.to_str())
                            .unwrap_or("lod"),
                        level,
                        ext
                    ));
                    if lod_path.exists() {
                        generated.push(lod_path);
                        continue;
                    }
                    let mut lod_mesh = mesh.clone();
                    let ops = vec![
                        MeshEditOp::Decimate {
                            target_triangles: target,
                            preserve_boundaries: true,
                            preserve_uv_seams: true,
                            uv_seam_threshold: 0.2,
                        },
                        MeshEditOp::RecomputeNormals,
                    ];
                    apply_mesh_edits(&mut lod_mesh, &ops)?;
                    let options = PlyExportOptions {
                        binary: false,
                        include_normals: !lod_mesh.normals.is_empty(),
                        include_colors: !lod_mesh.colors.is_empty(),
                        include_uvs: !lod_mesh.uvs.is_empty(),
                        comment: Some("TrueShot LOD".to_string()),
                    };
                    export_ply(&lod_mesh, &lod_path, &options)?;
                    generated.push(lod_path);
                }
            } else {
                let base_vertices = mesh.vertices.len();
                if base_vertices == 0 {
                    return Ok(Vec::new());
                }
                for (idx, ratio) in ratios.iter().enumerate() {
                    let level = (idx + 1) as u8;
                    let target = ((base_vertices as f32) * ratio).round() as usize;
                    if target >= base_vertices || target < 50 {
                        continue;
                    }
                    let lod_path = resolved.with_file_name(format!(
                        "{}.lod{}.{}",
                        resolved
                            .file_stem()
                            .and_then(|v| v.to_str())
                            .unwrap_or("lod"),
                        level,
                        ext
                    ));
                    if lod_path.exists() {
                        generated.push(lod_path);
                        continue;
                    }
                    if create_ply_lod(&resolved, &lod_path, target).is_ok() {
                        generated.push(lod_path);
                    }
                }
            }
        } else {
            let base_vertices = match ply_vertex_count(&resolved) {
                Ok(count) => count,
                Err(_) => return Ok(Vec::new()),
            };
            if base_vertices == 0 {
                return Ok(Vec::new());
            }
            for (idx, ratio) in ratios.iter().enumerate() {
                let level = (idx + 1) as u8;
                let target = ((base_vertices as f32) * ratio).round() as usize;
                if target >= base_vertices || target < 50 {
                    continue;
                }
                let lod_path = resolved.with_file_name(format!(
                    "{}.lod{}.{}",
                    resolved
                        .file_stem()
                        .and_then(|v| v.to_str())
                        .unwrap_or("lod"),
                    level,
                    ext
                ));
                if lod_path.exists() {
                    generated.push(lod_path);
                    continue;
                }
                if create_ply_lod(&resolved, &lod_path, target).is_ok() {
                    generated.push(lod_path);
                }
            }
        }
    } else if ext == "glb" {
        let mesh = match load_gltf_mesh(&resolved) {
            Ok(mesh) => mesh,
            Err(err) => {
                tracing::warn!("GLB LOD load failed: {}", err);
                return Ok(Vec::new());
            }
        };
        let base_faces = mesh.faces.len().max(1);
        for (idx, ratio) in ratios.iter().enumerate() {
            let level = (idx + 1) as u8;
            let target = ((base_faces as f32) * ratio).round() as usize;
            if target >= base_faces || target < 4 {
                continue;
            }
            let lod_path = resolved.with_file_name(format!(
                "{}.lod{}.{}",
                resolved
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("lod"),
                level,
                ext
            ));
            if lod_path.exists() {
                generated.push(lod_path);
                continue;
            }
            let mut lod_mesh = mesh.clone();
            let ops = vec![
                MeshEditOp::Decimate {
                    target_triangles: target,
                    preserve_boundaries: true,
                    preserve_uv_seams: true,
                    uv_seam_threshold: 0.2,
                },
                MeshEditOp::RecomputeNormals,
            ];
            apply_mesh_edits(&mut lod_mesh, &ops)?;
            export_glb(&lod_mesh, &lod_path)?;
            generated.push(lod_path);
        }
    }
    Ok(generated)
}

fn ply_vertex_count(path: &std::path::Path) -> anyhow::Result<usize> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut vertex_count = 0usize;
    let mut is_ascii = false;
    let mut element_depth = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("format ") {
            if trimmed.contains("ascii") {
                is_ascii = true;
            } else {
                return Err(anyhow!("PLY not ascii"));
            }
        } else if trimmed.starts_with("element ") {
            element_depth += 1;
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[1];
                let count = parts[2].parse::<usize>().unwrap_or(0);
                if name == "vertex" {
                    vertex_count = count;
                } else if count > 0 {
                    return Err(anyhow!("PLY has non-vertex elements"));
                }
            }
        } else if trimmed == "end_header" {
            break;
        }
    }
    if element_depth == 0 || !is_ascii {
        return Err(anyhow!("PLY header invalid"));
    }
    Ok(vertex_count)
}

fn create_ply_lod(
    base_path: &std::path::Path,
    lod_path: &std::path::Path,
    target_vertices: usize,
) -> anyhow::Result<()> {
    let file = std::fs::File::open(base_path)?;
    let mut reader = BufReader::new(file);
    let mut header_lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut vertex_count = 0usize;
    let mut is_ascii = false;
    let mut non_vertex_elements = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("format ") {
            if trimmed.contains("ascii") {
                is_ascii = true;
            } else {
                return Err(anyhow!("PLY not ascii"));
            }
        } else if trimmed.starts_with("element ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[1];
                let count = parts[2].parse::<usize>().unwrap_or(0);
                if name == "vertex" {
                    vertex_count = count;
                } else if count > 0 {
                    non_vertex_elements = true;
                }
            }
        }
        header_lines.push(line.clone());
        if trimmed == "end_header" {
            break;
        }
    }
    if !is_ascii || non_vertex_elements || vertex_count == 0 {
        return Err(anyhow!("PLY unsupported for LOD"));
    }
    let target = target_vertices.min(vertex_count).max(10);
    let step = (vertex_count as f64 / target as f64).ceil() as usize;
    let mut out = std::fs::File::create(lod_path)?;
    for mut hline in header_lines {
        if hline.trim_start().starts_with("element vertex ") {
            hline = format!("element vertex {}\n", target);
        }
        out.write_all(hline.as_bytes())?;
    }
    let mut written = 0usize;
    for idx in 0..vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if idx % step == 0 && written < target {
            out.write_all(line.as_bytes())?;
            written += 1;
        }
    }
    Ok(())
}

fn load_ply_ascii_mesh(path: &std::path::Path) -> anyhow::Result<Mesh> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut vertex_count = 0usize;
    let mut face_count = 0usize;
    let mut is_ascii = false;
    let mut vertex_properties: Vec<String> = Vec::new();
    let mut current_element: Option<String> = None;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("format ") {
            if trimmed.contains("ascii") {
                is_ascii = true;
            } else {
                return Err(anyhow!("PLY not ascii"));
            }
        } else if trimmed.starts_with("element ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[1];
                let count = parts[2].parse::<usize>().unwrap_or(0);
                current_element = Some(name.to_string());
                if name == "vertex" {
                    vertex_count = count;
                } else if name == "face" {
                    face_count = count;
                }
            }
        } else if trimmed.starts_with("property ") {
            if let Some(ref element) = current_element {
                if element == "vertex" {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if let Some(name) = parts.last() {
                        vertex_properties.push(name.to_string());
                    }
                }
            }
        } else if trimmed == "end_header" {
            break;
        }
    }
    if !is_ascii || vertex_count == 0 {
        return Err(anyhow!("PLY header invalid"));
    }
    let index_of = |name: &str| vertex_properties.iter().position(|p| p == name);
    let x_idx = index_of("x").ok_or_else(|| anyhow!("PLY missing x"))?;
    let y_idx = index_of("y").ok_or_else(|| anyhow!("PLY missing y"))?;
    let z_idx = index_of("z").ok_or_else(|| anyhow!("PLY missing z"))?;
    let nx_idx = index_of("nx");
    let ny_idx = index_of("ny");
    let nz_idx = index_of("nz");
    let u_idx = index_of("s").or_else(|| index_of("u"));
    let v_idx = index_of("t").or_else(|| index_of("v"));
    let r_idx = index_of("red");
    let g_idx = index_of("green");
    let b_idx = index_of("blue");

    let mut vertices = Vec::with_capacity(vertex_count);
    let mut normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    for _ in 0..vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() <= z_idx {
            continue;
        }
        let x = parts
            .get(x_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let y = parts
            .get(y_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let z = parts
            .get(z_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        vertices.push(na::Point3::new(x, y, z));

        if let (Some(nx_idx), Some(ny_idx), Some(nz_idx)) = (nx_idx, ny_idx, nz_idx) {
            let nx = parts
                .get(nx_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let ny = parts
                .get(ny_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let nz = parts
                .get(nz_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            normals.push(na::Vector3::new(nx, ny, nz));
        }

        if let (Some(u_idx), Some(v_idx)) = (u_idx, v_idx) {
            let u = parts
                .get(u_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let v = parts
                .get(v_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            uvs.push([u, v]);
        }

        if let (Some(r_idx), Some(g_idx), Some(b_idx)) = (r_idx, g_idx, b_idx) {
            let r = parts
                .get(r_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            let g = parts
                .get(g_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            let b = parts
                .get(b_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            colors.push([
                if r <= 1.0 { (r * 255.0) as u8 } else { r as u8 },
                if g <= 1.0 { (g * 255.0) as u8 } else { g as u8 },
                if b <= 1.0 { (b * 255.0) as u8 } else { b as u8 },
            ]);
        }
    }

    let mut faces = Vec::new();
    for _ in 0..face_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let count = parts[0].parse::<usize>().unwrap_or(0);
        if count < 3 || parts.len() < count + 1 {
            continue;
        }
        let mut indices: Vec<usize> = Vec::with_capacity(count);
        for idx in 0..count {
            let value = parts
                .get(idx + 1)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            indices.push(value);
        }
        for tri in 1..(indices.len() - 1) {
            faces.push(Face {
                vertices: [indices[0], indices[tri], indices[tri + 1]],
            });
        }
    }

    Ok(Mesh {
        vertices,
        colors,
        normals,
        uvs,
        faces,
    })
}

fn load_gltf_mesh(path: &std::path::Path) -> anyhow::Result<Mesh> {
    let (doc, buffers, _) = gltf::import(path)?;
    let mut vertices: Vec<na::Point3<f32>> = Vec::new();
    let mut normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let mut vertex_offset = 0usize;

    for mesh in doc.meshes() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .map(|iter| iter.collect())
                .unwrap_or_default();
            if positions.is_empty() {
                continue;
            }
            let prim_normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|iter| iter.collect())
                .unwrap_or_default();
            let prim_uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|iter| iter.into_f32().collect())
                .unwrap_or_default();
            let prim_colors: Vec<[f32; 4]> = reader
                .read_colors(0)
                .map(|iter| iter.into_rgba_f32().collect())
                .unwrap_or_default();
            let indices: Vec<u32> = if let Some(read_indices) = reader.read_indices() {
                read_indices.into_u32().collect()
            } else {
                (0..positions.len() as u32).collect()
            };

            for p in &positions {
                vertices.push(na::Point3::new(p[0], p[1], p[2]));
            }

            let should_fill_normals =
                !prim_normals.is_empty() && prim_normals.len() == positions.len();
            if !normals.is_empty() || should_fill_normals {
                for i in 0..positions.len() {
                    if should_fill_normals {
                        let n = prim_normals[i];
                        normals.push(na::Vector3::new(n[0], n[1], n[2]));
                    } else {
                        normals.push(na::Vector3::z());
                    }
                }
            }

            let should_fill_uvs = !prim_uvs.is_empty() && prim_uvs.len() == positions.len();
            if !uvs.is_empty() || should_fill_uvs {
                for i in 0..positions.len() {
                    if should_fill_uvs {
                        uvs.push([prim_uvs[i][0], prim_uvs[i][1]]);
                    } else {
                        uvs.push([0.0, 0.0]);
                    }
                }
            }

            let should_fill_colors =
                !prim_colors.is_empty() && prim_colors.len() == positions.len();
            if !colors.is_empty() || should_fill_colors {
                for i in 0..positions.len() {
                    if should_fill_colors {
                        let c = prim_colors[i];
                        colors.push([
                            (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                            (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                            (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                        ]);
                    } else {
                        colors.push([255, 255, 255]);
                    }
                }
            }

            for tri in indices.chunks(3) {
                if tri.len() != 3 {
                    continue;
                }
                faces.push(Face {
                    vertices: [
                        tri[0] as usize + vertex_offset,
                        tri[1] as usize + vertex_offset,
                        tri[2] as usize + vertex_offset,
                    ],
                });
            }

            vertex_offset += positions.len();
        }
    }

    Ok(Mesh {
        vertices,
        colors,
        normals,
        uvs,
        faces,
    })
}

async fn open_share_file(
    state: &AppState,
    project_id: &str,
    file_path: &std::path::Path,
) -> Result<NamedFile, HttpResponse> {
    let resolved = open_share_file_path(state, project_id, file_path).await?;
    NamedFile::open_async(resolved)
        .await
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))
}

async fn open_share_file_path(
    state: &AppState,
    project_id: &str,
    file_path: &std::path::Path,
) -> Result<std::path::PathBuf, HttpResponse> {
    let mut resolved = file_path.to_path_buf();
    if !resolved.exists() {
        let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            let key_store = project_key_store(state)?;
            let key = key_store
                .load_or_create(project_id)
                .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
            if let Ok(restored) = decrypt_file_in_place(&enc_path, &key) {
                resolved = restored;
            }
        }
    }

    if !resolved.exists() {
        return Err(HttpResponse::NotFound().body("File not found"));
    }
    Ok(resolved)
}

async fn stream_share_range(
    file_path: &std::path::Path,
    offset: u64,
    limit: Option<u64>,
    download: bool,
) -> HttpResponse {
    let file = match tokio::fs::File::open(file_path).await {
        Ok(file) => file,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let metadata = match file.metadata().await {
        Ok(meta) => meta,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };
    let file_size = metadata.len();
    if offset >= file_size {
        return HttpResponse::RangeNotSatisfiable()
            .append_header(("Content-Range", format!("bytes */{}", file_size)))
            .finish();
    }
    let mut end = file_size;
    if let Some(limit) = limit {
        end = end.min(offset.saturating_add(limit));
    }
    if end == 0 || end < offset {
        return HttpResponse::RangeNotSatisfiable()
            .append_header(("Content-Range", format!("bytes */{}", file_size)))
            .finish();
    }
    let length = end - offset;
    let mut file = file;
    if let Err(err) = file.seek(std::io::SeekFrom::Start(offset)).await {
        return HttpResponse::InternalServerError().body(err.to_string());
    }
    let stream = ReaderStream::new(file.take(length));
    let mime = mime_guess::from_path(file_path).first_or_octet_stream();
    let mut response = HttpResponse::PartialContent();
    response.append_header(("Content-Type", mime.to_string()));
    response.append_header((
        "Content-Range",
        format!("bytes {}-{}/{}", offset, end - 1, file_size),
    ));
    response.append_header(("Content-Length", length.to_string()));
    response.append_header(("Accept-Ranges", "bytes"));
    if download {
        response.append_header((actix_web::http::header::CONTENT_DISPOSITION, "attachment"));
    }
    response.streaming(stream)
}

fn project_key_store(state: &AppState) -> Result<ProjectKeyStore, HttpResponse> {
    let master_key = require_master_key(&state.config.privacy, &state.config.paths.projects_dir)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(ProjectKeyStore::new(
        &state.config.paths.projects_dir,
        master_key,
    ))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn request_metadata(req: &HttpRequest) -> (Option<String>, Option<String>, Option<String>) {
    let ip = req
        .connection_info()
        .realip_remote_addr()
        .map(|v| v.to_string());
    let user_agent = req
        .headers()
        .get(actix_web::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|v| v.to_string());
    let referrer = req
        .headers()
        .get(actix_web::http::header::REFERER)
        .and_then(|value| value.to_str().ok())
        .map(|v| v.to_string());
    (ip, user_agent, referrer)
}

fn parse_range_header(value: &str, file_size: u64) -> Result<(u64, u64), ()> {
    let value = value.trim();
    if !value.starts_with("bytes=") {
        return Err(());
    }
    let ranges = value.trim_start_matches("bytes=").split(',');
    let first = ranges.into_iter().next().ok_or(())?.trim();
    let mut parts = first.splitn(2, '-');
    let start_str = parts.next().unwrap_or("").trim();
    let end_str = parts.next().unwrap_or("").trim();
    if start_str.is_empty() && end_str.is_empty() {
        return Err(());
    }
    let (start, end) = if start_str.is_empty() {
        let suffix: u64 = end_str.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = file_size.saturating_sub(suffix);
        let end = file_size.saturating_sub(1);
        (start, end)
    } else {
        let start: u64 = start_str.parse().map_err(|_| ())?;
        let end: u64 = if end_str.is_empty() {
            file_size.saturating_sub(1)
        } else {
            end_str.parse().map_err(|_| ())?
        };
        (start, end)
    };
    if start > end || start >= file_size {
        return Err(());
    }
    Ok((start, end.min(file_size.saturating_sub(1))))
}

async fn log_share_access(
    state: &web::Data<AppState>,
    token: &str,
    req: &HttpRequest,
    event: &str,
    embed: bool,
    download: bool,
) {
    let (ip, user_agent, referrer) = request_metadata(req);
    let now = unix_timestamp();
    if let Err(err) = state
        .auth
        .record_share_access(
            token,
            event,
            now,
            ip.as_deref(),
            user_agent.as_deref(),
            referrer.as_deref(),
            embed,
            download,
        )
        .await
    {
        tracing::warn!("Failed to record share access: {}", err);
    }
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let actor = req
        .extensions()
        .get::<crate::auth::AuthContext>()
        .map(|ctx| ctx.sub.clone())
        .unwrap_or_else(|| "anonymous".to_string());
    let role = req
        .extensions()
        .get::<crate::auth::AuthContext>()
        .map(|ctx| format!("{:?}", ctx.role))
        .unwrap_or_else(|| "Unknown".to_string());
    let ip = req
        .connection_info()
        .realip_remote_addr()
        .map(|v| v.to_string());
    (actor, role, ip)
}

fn log_audit(_req: &HttpRequest, state: &web::Data<AppState>, event: AuditEvent) {
    let log = state.audit.clone();
    if let Err(err) = log.append(event) {
        tracing::warn!("Failed to write audit log: {}", err);
    }
}
