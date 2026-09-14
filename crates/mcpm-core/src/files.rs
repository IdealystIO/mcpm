//! Where an attachment's bytes live.
//!
//! The store keeps the RECORD of an attachment — what it is called,
//! what it is for, who added it to which feature or want — and hands
//! the bytes to a [`FileProvider`]. There are two: [`S3Files`] for a
//! deployment (S3 itself, or MinIO in the devcontainer, which speaks
//! the same protocol) and [`MemoryFiles`] for tests, which need the
//! store's rules exercised without an object store standing by.
//!
//! The provider is deliberately dumb: put, get, delete, and a
//! presigned link. Every rule about WHO may attach WHAT to WHICH
//! subject is the store's, inside the transaction that records it, as
//! every other invariant here is.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_core::future::BoxFuture;

use crate::error::{ErrorCode, McpmError};

type Result<T> = std::result::Result<T, McpmError>;

/// The object store behind attachments.
///
/// Boxed futures rather than an `async_trait` dependency: four methods
/// is not enough surface to earn a proc macro.
pub trait FileProvider: Send + Sync {
    /// Store `bytes` at `key`, replacing anything there.
    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: Vec<u8>,
        content_type: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// The whole object at `key`.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Remove the object at `key`. Removing what is not there is fine.
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>>;

    /// A URL that fetches the object without credentials, good for
    /// `ttl_secs`, served with `filename` as the download name.
    ///
    /// `None` when the provider cannot mint one — a caller then serves
    /// the bytes itself (the console host does; the MCP tool inlines
    /// text and otherwise says to ask the console).
    fn presign_get<'a>(
        &'a self,
        key: &'a str,
        ttl_secs: u32,
        filename: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>>;

    /// One line for a startup banner. Never a credential.
    fn describe(&self) -> String;
}

// ---------------------------------------------------------------------
// S3 / MinIO
// ---------------------------------------------------------------------

/// Which environment variables configure [`S3Files`], in the order
/// each is consulted. The `MINIO_*` names are what the devcontainer's
/// idealyst-managed sidecar exports; the `AWS_*` names are what a
/// deployment already has.
pub const S3_ENV: &[(&str, &[&str])] = &[
    ("bucket", &["MCPM_S3_BUCKET"]),
    ("endpoint", &["MCPM_S3_ENDPOINT", "MINIO_ENDPOINT"]),
    ("public endpoint (presigned links)", &["MCPM_S3_PUBLIC_ENDPOINT"]),
    ("region", &["MCPM_S3_REGION", "AWS_REGION", "AWS_DEFAULT_REGION"]),
    ("access key", &["MCPM_S3_ACCESS_KEY", "MINIO_ACCESS_KEY", "AWS_ACCESS_KEY_ID"]),
    ("secret key", &["MCPM_S3_SECRET_KEY", "MINIO_SECRET_KEY", "AWS_SECRET_ACCESS_KEY"]),
];

/// The bucket used when `MCPM_S3_BUCKET` is unset.
pub const DEFAULT_BUCKET: &str = "mcpm-attachments";

/// An S3-compatible object store: S3, MinIO, R2, anything that signs
/// requests the SigV4 way.
pub struct S3Files {
    /// The bucket every operation goes through.
    bucket: Box<s3::Bucket>,
    /// The bucket presigned links are minted against. The same as
    /// `bucket` unless a public endpoint was configured: inside a
    /// devcontainer the store is `http://minio:9000`, and a link with
    /// that host in it resolves to nothing in the reader's browser.
    /// A presigned URL signs the host, so it is minted against the one
    /// the reader will use.
    link_bucket: Box<s3::Bucket>,
    endpoint: String,
}

impl S3Files {
    /// Build from the environment (see [`S3_ENV`]), or `None` when
    /// nothing there names an object store — a deployment without
    /// attachments is a valid one, and the store says so at the moment
    /// somebody tries to attach rather than refusing to start.
    ///
    /// Configured means: a bucket, an endpoint, or an access key is
    /// set. An endpoint without credentials is refused rather than
    /// tried anonymously; a bucket alone falls through to the AWS
    /// credential chain (env, profile, instance role).
    pub fn from_env() -> Result<Option<S3Files>> {
        let var = |names: &[&str]| -> Option<String> {
            names
                .iter()
                .find_map(|n| std::env::var(n).ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let bucket = var(&["MCPM_S3_BUCKET"]);
        let endpoint = var(&["MCPM_S3_ENDPOINT", "MINIO_ENDPOINT"]);
        let access = var(&["MCPM_S3_ACCESS_KEY", "MINIO_ACCESS_KEY", "AWS_ACCESS_KEY_ID"]);
        let secret = var(&["MCPM_S3_SECRET_KEY", "MINIO_SECRET_KEY", "AWS_SECRET_ACCESS_KEY"]);
        if bucket.is_none() && endpoint.is_none() && access.is_none() {
            return Ok(None);
        }
        let region_name = var(&["MCPM_S3_REGION", "AWS_REGION", "AWS_DEFAULT_REGION"])
            .unwrap_or_else(|| "us-east-1".to_string());
        let public = var(&["MCPM_S3_PUBLIC_ENDPOINT"]);
        let bucket_name = bucket.unwrap_or_else(|| DEFAULT_BUCKET.to_string());

        let creds = match (&access, &secret) {
            (Some(a), Some(s)) => s3::creds::Credentials::new(Some(a), Some(s), None, None, None),
            (Some(_), None) | (None, Some(_)) => {
                return Err(config_error(
                    "an S3 access key was set without its secret (or the reverse); set both \
                     MCPM_S3_ACCESS_KEY and MCPM_S3_SECRET_KEY (or the MINIO_* / AWS_* pair).",
                ))
            }
            (None, None) if endpoint.is_some() => {
                return Err(config_error(
                    "an S3 endpoint was set with no credentials; set MCPM_S3_ACCESS_KEY and \
                     MCPM_S3_SECRET_KEY (or MINIO_ACCESS_KEY / MINIO_SECRET_KEY).",
                ))
            }
            (None, None) => s3::creds::Credentials::default(),
        }
        .map_err(|e| config_error(format!("S3 credentials: {e}")))?;

        let region = |endpoint: &Option<String>| match endpoint {
            Some(e) => s3::Region::Custom { region: region_name.clone(), endpoint: trim_slash(e) },
            None => region_name
                .parse()
                .unwrap_or(s3::Region::Custom { region: region_name.clone(), endpoint: String::new() }),
        };
        let make = |endpoint: &Option<String>| -> Result<Box<s3::Bucket>> {
            let b = s3::Bucket::new(&bucket_name, region(endpoint), creds.clone())
                .map_err(|e| config_error(format!("S3 bucket handle: {e}")))?;
            // Path style (`host/bucket/key`) is what MinIO serves and
            // what S3 still accepts; virtual-host style needs DNS for
            // the bucket name, which a local endpoint never has.
            Ok(if endpoint.is_some() { b.with_path_style() } else { b })
        };
        let bucket = make(&endpoint)?;
        let link_bucket = match &public {
            Some(_) => make(&public)?,
            None => make(&endpoint)?,
        };
        Ok(Some(S3Files {
            bucket,
            link_bucket,
            endpoint: endpoint.clone().unwrap_or_else(|| format!("aws:{region_name}")),
        }))
    }

    /// Make sure the bucket exists, creating it when the store is one
    /// we may create in (a custom endpoint — MinIO — rather than AWS,
    /// where a bucket is an account-level act somebody should do on
    /// purpose). Called once at startup so the first attach does not
    /// fail with a 404 nobody expects.
    pub async fn ensure_bucket(&self) -> Result<()> {
        let exists = self
            .bucket
            .exists()
            .await
            .map_err(|e| unavailable(format!("cannot reach the object store at {}: {e}", self.endpoint)))?;
        if exists {
            return Ok(());
        }
        if !matches!(self.bucket.region(), s3::Region::Custom { .. }) {
            return Err(config_error(format!(
                "bucket '{}' does not exist in {}; create it (this server only creates buckets \
                 on a custom endpoint such as MinIO).",
                self.bucket.name(),
                self.endpoint
            )));
        }
        s3::Bucket::create_with_path_style(
            &self.bucket.name(),
            self.bucket.region(),
            self.bucket.credentials().await.map_err(|e| unavailable(e.to_string()))?,
            s3::BucketConfiguration::private(),
        )
        .await
        .map_err(|e| unavailable(format!("cannot create bucket '{}': {e}", self.bucket.name())))?;
        Ok(())
    }
}

impl FileProvider for S3Files {
    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: Vec<u8>,
        content_type: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.bucket
                .put_object_with_content_type(key, &bytes, content_type)
                .await
                .map_err(|e| unavailable(format!("object store put failed: {e}")))?;
            Ok(())
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let data = self
                .bucket
                .get_object(key)
                .await
                .map_err(|e| unavailable(format!("object store get failed: {e}")))?;
            Ok(data.to_vec())
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.bucket
                .delete_object(key)
                .await
                .map_err(|e| unavailable(format!("object store delete failed: {e}")))?;
            Ok(())
        })
    }

    fn presign_get<'a>(
        &'a self,
        key: &'a str,
        ttl_secs: u32,
        filename: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            let mut queries = HashMap::new();
            queries.insert(
                "response-content-disposition".to_string(),
                format!("attachment; filename=\"{}\"", filename.replace('"', "")),
            );
            let url = self
                .link_bucket
                .presign_get(key, ttl_secs, Some(queries))
                .await
                .map_err(|e| unavailable(format!("cannot presign a link: {e}")))?;
            Ok(Some(url))
        })
    }

    fn describe(&self) -> String {
        format!("s3 bucket '{}' at {}", self.bucket.name(), self.endpoint)
    }
}

fn trim_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------
// In memory
// ---------------------------------------------------------------------

/// A provider that holds everything in a map. For tests; a deployment
/// that used it would lose every file on restart.
#[derive(Default)]
pub struct MemoryFiles {
    objects: Mutex<HashMap<String, (String, Vec<u8>)>>,
}

impl MemoryFiles {
    pub fn new() -> Arc<MemoryFiles> {
        Arc::new(MemoryFiles::default())
    }

    /// How many objects are held — what a test asserts after a remove.
    pub fn len(&self) -> usize {
        self.objects.lock().expect("memory files lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl FileProvider for MemoryFiles {
    fn put<'a>(
        &'a self,
        key: &'a str,
        bytes: Vec<u8>,
        content_type: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.objects
                .lock()
                .expect("memory files lock")
                .insert(key.to_string(), (content_type.to_string(), bytes));
            Ok(())
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.objects
                .lock()
                .expect("memory files lock")
                .get(key)
                .map(|(_, b)| b.clone())
                .ok_or_else(|| unavailable(format!("no object at {key}")))
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.objects.lock().expect("memory files lock").remove(key);
            Ok(())
        })
    }

    fn presign_get<'a>(
        &'a self,
        _key: &'a str,
        _ttl_secs: u32,
        _filename: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move { Ok(None) })
    }

    fn describe(&self) -> String {
        "in-memory files (nothing survives a restart)".to_string()
    }
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// The object store is misconfigured. An operator's problem, said in
/// operator's terms.
pub fn config_error(message: impl Into<String>) -> McpmError {
    McpmError::new(
        ErrorCode::Internal,
        message,
        serde_json::Value::Null,
        "Fix the MCPM_S3_* / MINIO_* environment and restart.",
    )
}

/// The object store did not answer, or answered with a refusal.
pub fn unavailable(message: impl Into<String>) -> McpmError {
    McpmError::new(
        ErrorCode::Internal,
        message,
        serde_json::Value::Null,
        "The attachment record is untouched. Retry, or check the object store.",
    )
}

/// No provider is installed on this server.
pub fn no_provider() -> McpmError {
    McpmError::new(
        ErrorCode::Internal,
        "This server has no file store configured, so attachments cannot be stored or read.",
        serde_json::Value::Null,
        "Set MCPM_S3_ENDPOINT / MCPM_S3_ACCESS_KEY / MCPM_S3_SECRET_KEY (the devcontainer's \
         MINIO_* variables also work) and restart the server.",
    )
}
