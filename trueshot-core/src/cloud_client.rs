//! S3/R2 Cloud Storage Client
//!
//! Replaces shell-out rsync with native Rust S3 implementation.

use anyhow::{Context, Result};
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use std::fs;
use std::path::Path;

pub trait CloudStorage {
    fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()>;
    fn upload_directory(&self, local_dir: &Path, remote_prefix: &str) -> Result<()>;
    fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<()>;
}

pub struct S3Client {
    bucket: Bucket,
}

impl S3Client {
    pub fn new(bucket_name: &str, region_name: &str, endpoint: Option<&str>) -> Result<Self> {
        let region = if let Some(ep) = endpoint {
            Region::Custom {
                region: region_name.to_string(),
                endpoint: ep.to_string(),
            }
        } else {
            match region_name {
                "us-east-1" => Region::UsEast1,
                _ => Region::Custom {
                    region: region_name.to_string(),
                    endpoint: "https://s3.amazonaws.com".to_string(),
                },
            }
        };

        let credentials = Credentials::default()?;
        let bucket = Bucket::new(bucket_name, region, credentials)?;

        Ok(Self { bucket })
    }

    pub fn new_with_credentials(
        bucket_name: &str,
        region_name: &str,
        endpoint: Option<&str>,
        access_key: &str,
        secret_key: &str,
        session_token: Option<&str>,
    ) -> Result<Self> {
        let region = if let Some(ep) = endpoint {
            Region::Custom {
                region: region_name.to_string(),
                endpoint: ep.to_string(),
            }
        } else {
            match region_name {
                "us-east-1" => Region::UsEast1,
                _ => Region::Custom {
                    region: region_name.to_string(),
                    endpoint: "https://s3.amazonaws.com".to_string(),
                },
            }
        };

        let credentials = Credentials::new(
            Some(access_key),
            Some(secret_key),
            session_token,
            None,
            None,
        )?;
        let bucket = Bucket::new(bucket_name, region, credentials)?;

        Ok(Self { bucket })
    }
}

impl CloudStorage for S3Client {
    fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        tracing::info!(
            "Uploading {:?} to s3://{}/{}",
            local_path,
            self.bucket.name,
            remote_path
        );

        let content = fs::read(local_path).context("Failed to read local file")?;

        // Blocking call using tokio runtime
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(self.bucket.put_object(remote_path, &content))
            .context("Failed to upload info S3")?;

        Ok(())
    }

    fn upload_directory(&self, local_dir: &Path, remote_prefix: &str) -> Result<()> {
        tracing::info!(
            "Syncing {:?} to s3://{}/{}",
            local_dir,
            self.bucket.name,
            remote_prefix
        );
        for entry in walkdir::WalkDir::new(local_dir) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let rel_path = entry.path().strip_prefix(local_dir)?;
                let remote = format!("{}/{}", remote_prefix, rel_path.display());
                self.upload_file(entry.path(), &remote)?;
            }
        }
        Ok(())
    }

    fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        tracing::info!(
            "Downloading s3://{}/{} to {:?}",
            self.bucket.name,
            remote_path,
            local_path
        );

        let rt = tokio::runtime::Runtime::new()?;
        let data = rt
            .block_on(self.bucket.get_object(remote_path))
            .context("Failed to download from S3")?
            .to_vec();
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, data)?;
        Ok(())
    }
}
