use actix_web::body::EitherBody;
use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::{
    HeaderMap, HeaderName, HeaderValue, ACCESS_CONTROL_ALLOW_CREDENTIALS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, CACHE_CONTROL, CONTENT_TYPE,
    RETRY_AFTER, VARY,
};
use actix_web::{Error, HttpMessage, HttpResponse};
use futures::future::{ready, LocalBoxFuture, Ready};
use serde::Serialize;
use std::task::{Context, Poll};
use uuid::Uuid;

pub const CORRELATION_HEADER: &str = "x-correlation-id";

#[derive(Clone)]
pub struct CorrelationId(String);

impl CorrelationId {
    pub fn from_request(request: &actix_web::HttpRequest) -> Option<String> {
        request
            .extensions()
            .get::<Self>()
            .map(|value| value.0.clone())
    }
}

pub fn log_redacted_failure(request: &actix_web::HttpRequest, operation: &'static str) {
    let correlation_id =
        CorrelationId::from_request(request).unwrap_or_else(|| "unavailable".to_string());
    let route = request
        .match_pattern()
        .unwrap_or_else(|| "<unmatched>".to_string());
    tracing::warn!(
        correlation_id,
        method = %request.method(),
        route,
        operation,
        "request operation failed"
    );
}

#[derive(Debug, Serialize)]
struct PublicErrorEnvelope<'a> {
    error: PublicErrorBody<'a>,
}

#[derive(Debug, Serialize)]
struct PublicErrorBody<'a> {
    code: &'a str,
    message: &'a str,
    correlation_id: &'a str,
}

pub struct OpaqueServerErrors;

impl<S, B> Transform<S, ServiceRequest> for OpaqueServerErrors
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Transform = OpaqueServerErrorsMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(OpaqueServerErrorsMiddleware { service }))
    }
}

pub struct OpaqueServerErrorsMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for OpaqueServerErrorsMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(context)
    }

    fn call(&self, request: ServiceRequest) -> Self::Future {
        let correlation_id = Uuid::new_v4().to_string();
        request
            .extensions_mut()
            .insert(CorrelationId(correlation_id.clone()));
        let method = request.method().clone();
        let future = self.service.call(request);

        Box::pin(async move {
            match future.await {
                Ok(mut response) if !response.status().is_server_error() => {
                    insert_correlation_header(response.headers_mut(), &correlation_id);
                    Ok(response.map_into_left_body())
                }
                Ok(response) => {
                    let status = response.status();
                    let safe_headers = SafeFailureHeaders::from_headers(response.headers());
                    let route = response
                        .request()
                        .match_pattern()
                        .unwrap_or_else(|| "<unmatched>".to_string());
                    tracing::error!(
                        correlation_id,
                        method = %method,
                        route,
                        status = status.as_u16(),
                        "request returned an internal failure"
                    );
                    let (request, _) = response.into_parts();
                    Ok(ServiceResponse::new(
                        request,
                        opaque_response(status, &correlation_id, safe_headers),
                    )
                    .map_into_right_body())
                }
                Err(error) => {
                    tracing::error!(
                        correlation_id,
                        method = %method,
                        "request service failed before producing a response"
                    );
                    Err(error)
                }
            }
        })
    }
}

#[derive(Default)]
struct SafeFailureHeaders {
    retry_after: Option<HeaderValue>,
    allow_origin: Option<HeaderValue>,
    allow_credentials: Option<HeaderValue>,
    expose_headers: Option<HeaderValue>,
    vary: Option<HeaderValue>,
}

impl SafeFailureHeaders {
    fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            retry_after: headers.get(RETRY_AFTER).cloned(),
            allow_origin: headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).cloned(),
            allow_credentials: headers.get(ACCESS_CONTROL_ALLOW_CREDENTIALS).cloned(),
            expose_headers: headers.get(ACCESS_CONTROL_EXPOSE_HEADERS).cloned(),
            vary: headers.get(VARY).cloned(),
        }
    }

    fn apply(self, builder: &mut actix_web::HttpResponseBuilder) {
        for (name, value) in [
            (RETRY_AFTER, self.retry_after),
            (ACCESS_CONTROL_ALLOW_ORIGIN, self.allow_origin),
            (ACCESS_CONTROL_ALLOW_CREDENTIALS, self.allow_credentials),
            (ACCESS_CONTROL_EXPOSE_HEADERS, self.expose_headers),
            (VARY, self.vary),
        ] {
            if let Some(value) = value {
                builder.insert_header((name, value));
            }
        }
    }
}

fn opaque_response(
    status: actix_web::http::StatusCode,
    correlation_id: &str,
    safe_headers: SafeFailureHeaders,
) -> HttpResponse {
    let (code, message) = match status.as_u16() {
        501 => ("unsupported_operation", "The operation is not supported"),
        502 => ("upstream_error", "An upstream service failed"),
        503 => (
            "service_unavailable",
            "The service is temporarily unavailable",
        ),
        504 => ("upstream_timeout", "An upstream service timed out"),
        _ => ("internal_error", "The request could not be completed"),
    };
    let mut builder = HttpResponse::build(status);
    builder.insert_header((CONTENT_TYPE, "application/json"));
    builder.insert_header((CACHE_CONTROL, "no-store"));
    if let Ok(value) = HeaderValue::from_str(correlation_id) {
        builder.insert_header((HeaderName::from_static(CORRELATION_HEADER), value));
    }
    safe_headers.apply(&mut builder);
    builder.json(PublicErrorEnvelope {
        error: PublicErrorBody {
            code,
            message,
            correlation_id,
        },
    })
}

fn insert_correlation_header(headers: &mut actix_web::http::header::HeaderMap, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(CORRELATION_HEADER), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{error, http::StatusCode, test, web, App};

    async fn text_leak() -> HttpResponse {
        HttpResponse::InternalServerError()
            .insert_header((RETRY_AFTER, "17"))
            .insert_header((ACCESS_CONTROL_ALLOW_ORIGIN, "https://app.example"))
            .insert_header(("x-internal-database-host", "db.private"))
            .body("sqlite failure at /Users/operator/private.db token=secret")
    }

    async fn json_leak() -> HttpResponse {
        HttpResponse::BadGateway().json(serde_json::json!({
            "provider_error": "oauth_secret=private"
        }))
    }

    async fn service_error() -> Result<HttpResponse, Error> {
        Err(error::ErrorInternalServerError(
            "panic context /private/project",
        ))
    }

    async fn unsupported_operation() -> HttpResponse {
        HttpResponse::NotImplemented().body("adapter internals")
    }

    async fn validation_error() -> HttpResponse {
        HttpResponse::BadRequest().body("invalid aperture")
    }

    #[actix_web::test]
    async fn all_server_failures_are_opaque_and_correlated() {
        let app = test::init_service(
            App::new()
                .wrap(OpaqueServerErrors)
                .route("/text", web::get().to(text_leak))
                .route("/json", web::get().to(json_leak))
                .route("/service", web::get().to(service_error))
                .route("/unsupported", web::get().to(unsupported_operation))
                .route("/validation", web::get().to(validation_error)),
        )
        .await;

        for (path, status, code) in [
            ("/text", StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            ("/json", StatusCode::BAD_GATEWAY, "upstream_error"),
            (
                "/service",
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
            ),
            (
                "/unsupported",
                StatusCode::NOT_IMPLEMENTED,
                "unsupported_operation",
            ),
        ] {
            let response =
                test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
            assert_eq!(response.status(), status);
            let header = response
                .headers()
                .get(CORRELATION_HEADER)
                .expect("correlation header")
                .to_str()
                .expect("correlation header text")
                .to_string();
            Uuid::parse_str(&header).expect("UUID correlation ID");
            let body: serde_json::Value = test::read_body_json(response).await;
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["correlation_id"], header);
            assert_eq!(
                body.as_object().map(serde_json::Map::len),
                Some(1),
                "the public envelope has one top-level field"
            );
            assert_eq!(
                body["error"].as_object().map(serde_json::Map::len),
                Some(3),
                "the public error has an exact stable schema"
            );
            let encoded = body.to_string();
            for secret in ["sqlite", "/Users/", "token=", "oauth_secret", "/private/"] {
                assert!(!encoded.contains(secret));
            }
            assert!(!header.contains("secret"));
        }

        let response =
            test::call_service(&app, test::TestRequest::get().uri("/text").to_request()).await;
        assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "17");
        assert_eq!(
            response.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "https://app.example"
        );
        assert!(!response.headers().contains_key("x-internal-database-host"));
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let response = test::call_service(
            &app,
            test::TestRequest::get().uri("/validation").to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().contains_key(CORRELATION_HEADER));
        assert_eq!(test::read_body(response).await, "invalid aperture");
    }

    #[actix_web::test]
    async fn sensitive_share_and_oauth_failures_do_not_construct_public_error_bodies() {
        let share = include_str!("api/share.rs");
        let storage = include_str!("api/storage.rs");
        let concrete_path_call = ["request", ".path()"].concat();
        let concrete_trace_path_call = ["req", ".path().to_string()"].concat();

        assert!(!share.contains("InternalServerError().body(err.to_string())"));
        assert!(!storage.contains("HttpResponse::BadGateway().body"));
        assert!(!include_str!("public_error.rs").contains(&concrete_path_call));
        assert!(!include_str!("trace_middleware.rs").contains(&concrete_trace_path_call));
        for sensitive_template in [
            "Token exchange failed:",
            "Token parse failed:",
            "User info failed:",
            "Failed to persist tokens:",
            "Failed to persist credentials:",
        ] {
            assert!(!storage.contains(sensitive_template));
        }
    }
}
