use std::time::Duration;

use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Builder as S3ConfigBuilder, IdentityCache},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use chrono::{DateTime, Utc};
use secrecy::ExposeSecret;
use uuid::Uuid;

use crate::config::R2Config;

/// Well-known filename for the payload tarball stored in each review folder.
pub const PAYLOAD_FILENAME: &str = "payload.tar.gz";

/// Lifetime of presigned read URLs handed to the browser.
///
/// The frontend caches these for 4 minutes (`SAS_URL_TTL_MS` in
/// `packages/web-core/src/shared/lib/remoteApi.ts`). Keeping the server-side
/// expiry above that leaves a safety margin, so a cached URL can never be
/// served after it has already expired.
pub const READ_URL_EXPIRY: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct R2Service {
    client: Client,
    bucket: String,
    presign_expiry: Duration,
}

#[derive(Debug)]
pub struct PresignedUpload {
    pub upload_url: String,
    pub object_key: String,
    /// Folder path in R2 (e.g., "reviews/{review_id}") - this is stored in the database.
    pub folder_path: String,
    pub expires_at: DateTime<Utc>,
}

/// Presigned PUT for an arbitrary object key.
#[derive(Debug)]
pub struct PresignedPut {
    pub upload_url: String,
    pub object_key: String,
    pub expires_at: DateTime<Utc>,
}

/// Metadata for a stored object.
#[derive(Debug)]
pub struct ObjectProperties {
    pub content_length: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum R2Error {
    #[error("presign config error: {0}")]
    PresignConfig(String),
    #[error("presign error: {0}")]
    Presign(String),
    #[error("upload error: {0}")]
    Upload(String),
    #[error("download error: {0}")]
    Download(String),
    #[error("delete error: {0}")]
    Delete(String),
    #[error("metadata error: {0}")]
    Metadata(String),
    #[error("object not found: {0}")]
    NotFound(String),
}

impl R2Service {
    pub fn new(config: &R2Config) -> Self {
        let credentials = Credentials::new(
            &config.access_key_id,
            config.secret_access_key.expose_secret(),
            None,
            None,
            "r2-static",
        );

        let s3_config =
            S3ConfigBuilder::new()
                .region(aws_sdk_s3::config::Region::new("auto"))
                .endpoint_url(&config.endpoint)
                .credentials_provider(credentials)
                .force_path_style(true)
                .stalled_stream_protection(
                    aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
                )
                .identity_cache(IdentityCache::no_cache())
                .build();

        let client = Client::from_conf(s3_config);

        Self {
            client,
            bucket: config.bucket.clone(),
            presign_expiry: Duration::from_secs(config.presign_expiry_secs),
        }
    }

    fn presigning_config(expires_in: Duration) -> Result<PresigningConfig, R2Error> {
        PresigningConfig::builder()
            .expires_in(expires_in)
            .build()
            .map_err(|e| R2Error::PresignConfig(e.to_string()))
    }

    fn expires_at(expires_in: Duration) -> DateTime<Utc> {
        Utc::now()
            + chrono::Duration::from_std(expires_in).unwrap_or_else(|_| chrono::Duration::hours(1))
    }

    // ---------------------------------------------------------------------
    // Generic object operations (arbitrary keys)
    // ---------------------------------------------------------------------

    /// Presign a PUT for an arbitrary object key.
    ///
    /// Deliberately does **not** sign `Content-Type`. Attachment uploads are
    /// presigned before the MIME type is known (`InitUploadRequest` carries no
    /// content type), and SigV4 enforces any header it signs byte-for-byte — a
    /// mismatch between what the server signs and what the browser sends is a
    /// 403. The stored type is recorded in `blobs.mime_type` instead and
    /// applied on read via [`Self::create_presigned_read`].
    pub async fn create_presigned_put(&self, object_key: &str) -> Result<PresignedPut, R2Error> {
        let presigned = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(object_key)
            .presigned(Self::presigning_config(self.presign_expiry)?)
            .await
            .map_err(|e| R2Error::Presign(e.to_string()))?;

        Ok(PresignedPut {
            upload_url: presigned.uri().to_string(),
            object_key: object_key.to_string(),
            expires_at: Self::expires_at(self.presign_expiry),
        })
    }

    /// Presign a GET for an arbitrary object key.
    ///
    /// `content_type` sets `response-content-type` so the browser renders the
    /// object correctly (inline images rather than downloads) even though the
    /// stored object has no content type of its own — see
    /// [`Self::create_presigned_put`].
    pub async fn create_presigned_read(
        &self,
        object_key: &str,
        content_type: Option<&str>,
    ) -> Result<String, R2Error> {
        let mut request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(object_key);

        if let Some(ct) = content_type {
            request = request.response_content_type(ct);
        }

        let presigned = request
            .presigned(Self::presigning_config(READ_URL_EXPIRY)?)
            .await
            .map_err(|e| R2Error::Presign(e.to_string()))?;

        Ok(presigned.uri().to_string())
    }

    /// Upload bytes to an arbitrary object key (server-side upload).
    pub async fn put_object(
        &self,
        object_key: &str,
        data: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<(), R2Error> {
        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(object_key)
            .body(ByteStream::from(data));

        if let Some(ct) = content_type {
            request = request.content_type(ct);
        }

        request
            .send()
            .await
            .map_err(|e| R2Error::Upload(e.to_string()))?;

        Ok(())
    }

    /// Fetch an object's bytes.
    pub async fn get_object_bytes(&self, object_key: &str) -> Result<Vec<u8>, R2Error> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|e| R2Error::Download(e.to_string()))?;

        let bytes = output
            .body
            .collect()
            .await
            .map_err(|e| R2Error::Download(e.to_string()))?
            .into_bytes()
            .to_vec();

        if bytes.is_empty() {
            return Err(R2Error::NotFound(object_key.to_string()));
        }

        Ok(bytes)
    }

    /// Read an object's metadata without downloading it.
    pub async fn head_object(&self, object_key: &str) -> Result<ObjectProperties, R2Error> {
        let output = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|e| R2Error::Metadata(e.to_string()))?;

        Ok(ObjectProperties {
            content_length: output.content_length().unwrap_or(0),
        })
    }

    /// Delete an object. Succeeds if the object is already absent.
    pub async fn delete_object(&self, object_key: &str) -> Result<(), R2Error> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|e| R2Error::Delete(e.to_string()))?;

        Ok(())
    }

    // ---------------------------------------------------------------------
    // Review payloads (thin wrappers over the generic operations)
    // ---------------------------------------------------------------------

    fn review_paths(review_id: Uuid) -> (String, String) {
        let folder_path = format!("reviews/{review_id}");
        let object_key = format!("{folder_path}/{PAYLOAD_FILENAME}");
        (folder_path, object_key)
    }

    pub async fn create_presigned_upload(
        &self,
        review_id: Uuid,
        content_type: Option<&str>,
    ) -> Result<PresignedUpload, R2Error> {
        let (folder_path, object_key) = Self::review_paths(review_id);

        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&object_key);

        if let Some(ct) = content_type {
            request = request.content_type(ct);
        }

        let presigned = request
            .presigned(Self::presigning_config(self.presign_expiry)?)
            .await
            .map_err(|e| R2Error::Presign(e.to_string()))?;

        Ok(PresignedUpload {
            upload_url: presigned.uri().to_string(),
            object_key,
            folder_path,
            expires_at: Self::expires_at(self.presign_expiry),
        })
    }

    /// Upload a review payload directly to R2 (for server-side uploads).
    ///
    /// Returns the folder path (e.g., "reviews/{review_id}") to store in the database.
    pub async fn upload_bytes(&self, review_id: Uuid, data: Vec<u8>) -> Result<String, R2Error> {
        let (folder_path, object_key) = Self::review_paths(review_id);
        self.put_object(&object_key, data, Some("application/gzip"))
            .await?;
        Ok(folder_path)
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::*;

    fn test_service() -> R2Service {
        R2Service::new(&R2Config {
            access_key_id: "test-access-key".to_string(),
            secret_access_key: SecretString::new("test-secret-key".into()),
            endpoint: "https://accountid.r2.cloudflarestorage.com".to_string(),
            bucket: "agent-deck".to_string(),
            presign_expiry_secs: 3600,
        })
    }

    #[test]
    fn review_paths_are_stable() {
        let review_id = Uuid::nil();
        let (folder, key) = R2Service::review_paths(review_id);
        assert_eq!(folder, format!("reviews/{review_id}"));
        assert_eq!(key, format!("reviews/{review_id}/{PAYLOAD_FILENAME}"));
    }

    #[tokio::test]
    async fn presigned_put_targets_the_requested_key() {
        let url = test_service()
            .create_presigned_put("attachments/proj/abc_file.png")
            .await
            .expect("presign should succeed")
            .upload_url;

        assert!(url.contains("attachments/proj/abc_file.png"), "url: {url}");
        assert!(url.contains("X-Amz-Signature"), "url: {url}");
        // Content-Type must not be signed, or the browser PUT would have to
        // match it byte-for-byte and would otherwise 403.
        assert!(
            !url.to_lowercase().contains("content-type"),
            "presigned PUT must not sign Content-Type: {url}"
        );
    }

    #[tokio::test]
    async fn presigned_read_applies_response_content_type() {
        let url = test_service()
            .create_presigned_read("attachments/proj/abc_file.png", Some("image/png"))
            .await
            .expect("presign should succeed");

        assert!(url.contains("response-content-type"), "url: {url}");
        assert!(url.contains("image%2Fpng"), "url: {url}");
    }

    #[tokio::test]
    async fn presigned_read_without_content_type_omits_override() {
        let url = test_service()
            .create_presigned_read("attachments/proj/abc_file.bin", None)
            .await
            .expect("presign should succeed");

        assert!(!url.contains("response-content-type"), "url: {url}");
    }
}
