//! File storage: the local directory or S3 of `FileSettings`, as Mattermost's `filestore` drivers.

// Based on Mattermost server/platform/shared/filestore, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use std::sync::Arc;

use bytes::Bytes;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};

use crate::config::Config;

pub struct FileStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl FileStore {
    pub fn new(cfg: &Config) -> anyhow::Result<FileStore> {
        let f = |k: &str| cfg.str(&format!("FileSettings.{k}")).to_string();
        if f("DriverName") == "amazons3" {
            let ssl = cfg.bool("FileSettings.AmazonS3SSL");
            let endpoint = format!("{}://{}", if ssl { "https" } else { "http" }, f("AmazonS3Endpoint"));
            let mut b = object_store::aws::AmazonS3Builder::from_env()
                .with_bucket_name(f("AmazonS3Bucket"))
                .with_region(if f("AmazonS3Region").is_empty() { "us-east-1".into() } else { f("AmazonS3Region") })
                .with_endpoint(endpoint)
                .with_allow_http(!ssl);
            if !f("AmazonS3AccessKeyId").is_empty() {
                b = b.with_access_key_id(f("AmazonS3AccessKeyId")).with_secret_access_key(f("AmazonS3SecretAccessKey"));
            }
            if f("AmazonS3Endpoint") != "s3.amazonaws.com" {
                b = b.with_virtual_hosted_style_request(false);
            }
            return Ok(FileStore { store: Arc::new(b.build()?), prefix: f("AmazonS3PathPrefix") });
        }
        let dir = f("Directory");
        let dir = if dir.is_empty() { "./data/".to_string() } else { dir };
        std::fs::create_dir_all(&dir)?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(&dir)?.with_automatic_cleanup(true);
        Ok(FileStore { store: Arc::new(store), prefix: String::new() })
    }

    fn path(&self, p: &str) -> Path {
        if self.prefix.is_empty() {
            Path::from(p)
        } else {
            Path::from(format!("{}/{p}", self.prefix.trim_end_matches('/')))
        }
    }

    pub async fn read(&self, p: &str) -> anyhow::Result<Bytes> {
        Ok(self.store.get(&self.path(p)).await?.bytes().await?)
    }

    /// The bytes `range` of a file, and its whole size.
    pub async fn read_range(&self, p: &str, range: std::ops::Range<u64>) -> anyhow::Result<Bytes> {
        Ok(self.store.get_range(&self.path(p), range).await?)
    }

    pub async fn size(&self, p: &str) -> anyhow::Result<u64> {
        Ok(self.store.head(&self.path(p)).await?.size)
    }

    pub async fn exists(&self, p: &str) -> bool {
        self.store.head(&self.path(p)).await.is_ok()
    }

    pub async fn write(&self, p: &str, data: Bytes) -> anyhow::Result<()> {
        self.store.put(&self.path(p), PutPayload::from_bytes(data)).await?;
        Ok(())
    }

    pub async fn remove(&self, p: &str) -> anyhow::Result<()> {
        self.store.delete(&self.path(p)).await?;
        Ok(())
    }
}
