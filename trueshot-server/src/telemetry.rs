use crate::config::AppConfig;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{self, Sampler, Tracer};
use opentelemetry_sdk::Resource;

#[derive(Debug, Clone)]
pub struct TelemetrySettings {
    pub enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub sample_ratio: f64,
    pub service_name: String,
    pub service_version: String,
}

impl TelemetrySettings {
    pub fn from_config(config: &AppConfig, is_production: bool) -> Self {
        let enabled = config.server.telemetry_enabled.unwrap_or(is_production);
        let sample_ratio = config
            .server
            .telemetry_sample_ratio
            .unwrap_or(if is_production { 0.5 } else { 1.0 });
        let otlp_endpoint = config
            .server
            .telemetry_otlp_endpoint
            .clone()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok());
        let service_name = config
            .server
            .telemetry_service_name
            .clone()
            .unwrap_or_else(|| "trueshot-server".to_string());
        let service_version = config
            .server
            .telemetry_service_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

        Self {
            enabled,
            otlp_endpoint,
            sample_ratio,
            service_name,
            service_version,
        }
    }
}

pub fn init_tracer(settings: &TelemetrySettings) -> anyhow::Result<Option<Tracer>> {
    if !settings.enabled {
        return Ok(None);
    }

    let mut exporter = opentelemetry_otlp::new_exporter().tonic();
    if let Some(endpoint) = settings.otlp_endpoint.as_ref() {
        exporter = exporter.with_endpoint(endpoint);
    }

    let trace_config = trace::config()
        .with_sampler(Sampler::TraceIdRatioBased(
            settings.sample_ratio.clamp(0.0, 1.0),
        ))
        .with_resource(Resource::new(vec![
            KeyValue::new("service.name", settings.service_name.clone()),
            KeyValue::new("service.version", settings.service_version.clone()),
        ]));

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(trace_config)
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    Ok(Some(tracer))
}
