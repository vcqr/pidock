//! Minimal S3 (RustFS-compatible) SigV4 client: presigned PUT/GET URLs and
//! bucket creation. No aws-sdk dependency; region us-east-1 semantics.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct S3Client {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl S3Client {
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("PIDOCK_RUSTFS_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:7000".into()),
            bucket: std::env::var("PIDOCK_RUSTFS_BUCKET")
                .unwrap_or_else(|_| "pidock-attachments".into()),
            access_key: std::env::var("PIDOCK_RUSTFS_ACCESS_KEY")
                .unwrap_or_else(|_| "pidock".into()),
            secret_key: std::env::var("PIDOCK_RUSTFS_SECRET_KEY")
                .unwrap_or_else(|_| "pidock-secret".into()),
        }
    }

    fn uri_encoded(key: &str) -> String {
        // S3 path-style: encode every segment but keep '/'
        key.split('/')
            .map(|seg| {
                let mut out = String::new();
                for b in seg.bytes() {
                    match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                            out.push(b as char)
                        }
                        _ => out.push_str(&format!("%{b:02X}")),
                    }
                }
                out
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// presigned URL for PUT (upload) or GET (download)
    fn presign(&self, method: &str, key: &str, expires_secs: u64) -> String {
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let region = "us-east-1";
        let service = "s3";

        let credential = format!(
            "{}/{}/{}/{}/aws4_request",
            self.access_key, date_stamp, region, service
        );
        let host = self
            .endpoint
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .to_string();

        let canonical_uri = format!("/{}/{}", self.bucket, Self::uri_encoded(key));
        let mut query = vec![
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
            ("X-Amz-Credential", credential.clone()),
            ("X-Amz-Date", amz_date.clone()),
            ("X-Amz-Expires", expires_secs.to_string()),
            ("X-Amz-SignedHeaders", "host".to_string()),
        ];
        query.sort();
        let canonical_query = query
            .iter()
            .map(|(k, v)| format!("{}={}", k, url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let canonical_request = format!(
            "{}\n{}\n{}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            method, canonical_uri, canonical_query, host
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}/{}/{}/aws4_request\n{}",
            amz_date,
            date_stamp,
            region,
            service,
            sha256_hex(canonical_request.as_bytes())
        );
        // SigV4 derivation starts from "AWS4" + secret
        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, region.as_bytes());
        let k_service = hmac_sha256(&k_region, service.as_bytes());
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        format!(
            "{}/{}?{}&X-Amz-Signature={}",
            self.endpoint.trim_end_matches('/'),
            canonical_uri.trim_start_matches('/'),
            canonical_query,
            signature
        )
    }

    pub fn presign_put(&self, key: &str, expires_secs: u64) -> String {
        self.presign("PUT", key, expires_secs)
    }

    pub fn presign_get(&self, key: &str, expires_secs: u64) -> String {
        self.presign("GET", key, expires_secs)
    }

    /// create the bucket if missing (signed PUT on the bucket root)
    pub async fn ensure_bucket(&self) -> anyhow::Result<()> {
        let url = self.presign_put("", 60);
        let url = url.trim_end_matches('?').to_string();
        let resp = reqwest::Client::new().put(&url).send().await?;
        let status = resp.status();
        // 200 = created, 409 = already exists
        if status.is_success() || status.as_u16() == 409 {
            tracing::info!("s3 bucket '{}' ready", self.bucket);
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            // BucketAlreadyOwnedByYou etc. are fine
            if body.contains("BucketAlready") || status.as_u16() == 409 {
                Ok(())
            } else {
                anyhow::bail!("bucket create failed: {status} {body:.200}")
            }
        }
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
