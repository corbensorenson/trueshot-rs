use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::Error;
use futures::future::{ready, LocalBoxFuture, Ready};
use opentelemetry::trace::TraceContextExt;
use std::task::{Context, Poll};
use std::time::Instant;
use tracing::{field, info_span, Instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub struct TraceContext;

impl TraceContext {
    pub fn new() -> Self {
        Self
    }
}

impl<S, B> Transform<S, ServiceRequest> for TraceContext
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = TraceContextMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(TraceContextMiddleware { service }))
    }
}

pub struct TraceContextMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for TraceContextMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, ctx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(ctx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let method = req.method().to_string();
        let start = Instant::now();
        let span = info_span!(
            "http.request",
            http_method = %method,
            http_route = field::Empty,
            http_status = field::Empty,
            latency_ms = field::Empty,
            trace_id = field::Empty
        );
        let trace_id = trace_id_from_span(&span);
        if let Some(trace_id) = trace_id.as_ref() {
            span.record("trace_id", trace_id);
        }
        let fut = self.service.call(req);
        Box::pin(async move {
            let mut res = fut.instrument(span.clone()).await?;
            if let Some(route) = res.request().match_pattern() {
                span.record("http_route", route);
            }
            span.record("http_status", res.status().as_u16() as i64);
            span.record("latency_ms", start.elapsed().as_millis() as i64);
            if let Some(trace_id) = trace_id {
                let header_name = HeaderName::from_static("x-trace-id");
                if let Ok(value) = HeaderValue::from_str(&trace_id) {
                    res.headers_mut().insert(header_name, value);
                }
            }
            Ok(res)
        })
    }
}

fn trace_id_from_span(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let span_ref = context.span();
    let span_context = span_ref.span_context();
    if span_context.is_valid() {
        Some(span_context.trace_id().to_string())
    } else {
        None
    }
}
