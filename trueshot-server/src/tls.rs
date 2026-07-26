use anyhow::{anyhow, Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub fn load_rustls_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    let cert_file = File::open(cert_path)
        .with_context(|| format!("Failed to open TLS cert: {}", cert_path.display()))?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to read TLS certs from {}", cert_path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("No TLS certs found at {}", cert_path.display()));
    }

    let key_file = File::open(key_path)
        .with_context(|| format!("Failed to open TLS key: {}", key_path.display()))?;
    let mut key_reader = BufReader::new(key_file);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .with_context(|| format!("Failed to read TLS key from {}", key_path.display()))?
        .ok_or_else(|| anyhow!("No TLS private key found at {}", key_path.display()))?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("Invalid TLS cert/key pair")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}
