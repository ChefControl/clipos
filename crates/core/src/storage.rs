//! Blob Storage access through SAS URLs.
//!
//! Every blob operation, by the browser (upload, playback) or by us (HEAD, upload the
//! transcode, delete), goes through a short-lived SAS scoped to one blob. In Azure the SAS
//! is signed with a user delegation key fetched with the app's managed identity (shared
//! keys are disabled on the account). Locally it's signed with Azurite's account key.
//!
//! A failed request's error leaves out its URL (`reqwest::Error::without_url`): the URL
//! carries the SAS, and these errors end up in logs, `jobs.last_error` and `clips.error`.

use std::{path::Path, time::Duration};

use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use url::Url;

use crate::azure::Credential;

/// Storage REST version used for requests and SAS (`sv`).
const VERSION: &str = "2024-11-04";
const STORAGE_RESOURCE: &str = "https://storage.azure.com";
/// A delegation key is requested for this long and replaced when less than half is left.
const DELEGATION_KEY_LIFETIME: Duration = Duration::from_secs(2 * 24 * 3600);
/// Allows for clock skew between us and Azure.
const START_SKEW: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Originals,
    Playback,
    Posters,
    /// The killfeed detector's versioned models (`<name>/<version>/`).
    Models,
}

impl Container {
    pub const ALL: [Self; 4] = [Self::Originals, Self::Playback, Self::Posters, Self::Models];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Originals => "originals",
            Self::Playback => "playback",
            Self::Posters => "posters",
            Self::Models => "models",
        }
    }

    /// The container named `name` (`as_str`), if there is one.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == name)
    }
}

/// SAS permissions, in the canonical order Azure requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    /// Create + write: browser block uploads (Put Block, Put Block List) and Put Blob.
    Write,
    Delete,
}

impl Access {
    fn permissions(self) -> &'static str {
        match self {
            Self::Read => "r",
            Self::Write => "cw",
            Self::Delete => "d",
        }
    }
}

/// What a HEAD says about a blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobProps {
    pub size: u64,
    /// Changes whenever the blob is written. A read with `If-Match: {etag}` fails (412)
    /// if the blob was written since.
    pub etag: String,
}

/// A read with `If-Match` found the blob written since that ETag was taken (412).
#[derive(Debug, thiserror::Error)]
#[error("the blob was written after its ETag was taken")]
pub struct BlobChanged;

/// Response headers to override on reads (`rscd`, `rsct`), e.g. to force a download.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub content_disposition: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub account: String,
    /// Defaults to `https://{account}.blob.core.windows.net`. Azurite:
    /// `http://127.0.0.1:10000/devstoreaccount1`.
    pub blob_endpoint: Option<String>,
    /// Shared key (Azurite only). Without it, SAS are signed with a user delegation key.
    pub account_key: Option<String>,
}

pub struct Storage {
    account: String,
    /// Always ends with `/`.
    endpoint: Url,
    signer: Signer,
    http: reqwest::Client,
}

enum Signer {
    SharedKey(Vec<u8>),
    UserDelegation(Box<UserDelegation>),
}

struct UserDelegation {
    credential: Credential,
    key: RwLock<Option<DelegationKey>>,
}

#[derive(Debug, Clone)]
struct DelegationKey {
    oid: String,
    tid: String,
    start: String,
    expiry: String,
    expires_at: DateTime<Utc>,
    service: String,
    version: String,
    value: Vec<u8>,
}

impl Storage {
    pub fn new(config: StorageConfig) -> anyhow::Result<Self> {
        Self::with_credential(config, Credential::from_env)
    }

    /// `new`, getting Entra tokens (for user delegation keys) from `credential`.
    fn with_credential(
        config: StorageConfig,
        credential: impl FnOnce() -> Credential,
    ) -> anyhow::Result<Self> {
        let mut endpoint = config
            .blob_endpoint
            .unwrap_or_else(|| format!("https://{}.blob.core.windows.net", config.account));
        if !endpoint.ends_with('/') {
            endpoint.push('/');
        }
        let signer = match config.account_key {
            Some(key) => Signer::SharedKey(
                B64.decode(key)
                    .context("decoding the storage account key")?,
            ),
            None => Signer::UserDelegation(Box::new(UserDelegation {
                credential: credential(),
                key: RwLock::new(None),
            })),
        };
        Ok(Self {
            account: config.account,
            endpoint: Url::parse(&endpoint).context("parsing the blob endpoint")?,
            signer,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("static reqwest client config"),
        })
    }

    /// The blob service's origin (`https://{account}.blob.core.windows.net`), which the
    /// browser talks to for uploads and playback.
    pub fn origin(&self) -> String {
        self.endpoint.origin().ascii_serialization()
    }

    /// The blob's URL without a SAS.
    pub fn blob_url(&self, container: Container, blob: &str) -> Url {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .expect("http(s) URL")
            .pop_if_empty()
            .push(container.as_str())
            .extend(blob.split('/'));
        url
    }

    /// A SAS URL granting `access` to one blob for `ttl`.
    pub async fn sas_url(
        &self,
        container: Container,
        blob: &str,
        access: Access,
        ttl: Duration,
        overrides: &Overrides,
    ) -> anyhow::Result<Url> {
        let now = Utc::now();
        let start = fmt_time(now - START_SKEW);
        let mut expiry_at = now + ttl;
        let resource = format!("/blob/{}/{}/{}", self.account, container.as_str(), blob);
        let perms = access.permissions();
        let rscd = overrides.content_disposition.as_deref().unwrap_or("");
        let rsct = overrides.content_type.as_deref().unwrap_or("");
        let https = self.endpoint.scheme() == "https";
        let protocol = if https { "https" } else { "" };

        let mut query: Vec<(&str, String)> = Vec::new();
        let signature = match &self.signer {
            Signer::SharedKey(key) => {
                let expiry = fmt_time(expiry_at);
                let to_sign = [
                    perms, &start, &expiry, &resource, "", "", protocol, VERSION, "b", "", "", "",
                    rscd, "", "", rsct,
                ]
                .join("\n");
                query.extend([("st", start), ("se", expiry)]);
                sign(key, &to_sign)
            }
            Signer::UserDelegation(_) => {
                let key = self.delegation_key().await?;
                // A SAS can't outlive the key that signed it.
                expiry_at = expiry_at.min(key.expires_at);
                let expiry = fmt_time(expiry_at);
                let to_sign = [
                    perms,
                    &start,
                    &expiry,
                    &resource,
                    &key.oid,
                    &key.tid,
                    &key.start,
                    &key.expiry,
                    &key.service,
                    &key.version,
                    "",
                    "",
                    "",
                    "",
                    protocol,
                    VERSION,
                    "b",
                    "",
                    "",
                    "",
                    rscd,
                    "",
                    "",
                    rsct,
                ]
                .join("\n");
                query.extend([
                    ("st", start),
                    ("se", expiry),
                    ("skoid", key.oid.clone()),
                    ("sktid", key.tid.clone()),
                    ("skt", key.start.clone()),
                    ("ske", key.expiry.clone()),
                    ("sks", key.service.clone()),
                    ("skv", key.version.clone()),
                ]);
                sign(&key.value, &to_sign)
            }
        };

        query.extend([
            ("sv", VERSION.to_owned()),
            ("sr", "b".to_owned()),
            ("sp", perms.to_owned()),
        ]);
        if https {
            query.push(("spr", "https".to_owned()));
        }
        if !rscd.is_empty() {
            query.push(("rscd", rscd.to_owned()));
        }
        if !rsct.is_empty() {
            query.push(("rsct", rsct.to_owned()));
        }
        query.push(("sig", signature));

        let mut url = self.blob_url(container, blob);
        url.query_pairs_mut().extend_pairs(query);
        Ok(url)
    }

    /// Starts a GET (or HEAD) of a blob through a short-lived read SAS, passing `range`
    /// through, and returns the raw response for the caller to stream on. With `if_match`,
    /// Blob Storage answers 412 if the blob's ETag is no longer that one.
    pub async fn fetch(
        &self,
        container: Container,
        blob: &str,
        head: bool,
        range: Option<&str>,
        if_match: Option<&str>,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self
            .sas_url(
                container,
                blob,
                Access::Read,
                Duration::from_secs(3600),
                &Overrides::default(),
            )
            .await?;
        let mut req = if head {
            self.http.head(url)
        } else {
            self.http.get(url)
        };
        req = req.header("x-ms-version", VERSION);
        if let Some(range) = range {
            req = req.header(reqwest::header::RANGE, range);
        }
        if let Some(etag) = if_match {
            req = req.header(reqwest::header::IF_MATCH, etag);
        }
        req.send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("reading blob")
    }

    /// Downloads a blob to `path` in one streamed GET. Returns the number of bytes written.
    /// With `if_match`, fails with `BlobChanged` if the blob's ETag is no longer that one.
    pub async fn download_to_file(
        &self,
        container: Container,
        blob: &str,
        path: &Path,
        if_match: Option<&str>,
    ) -> anyhow::Result<u64> {
        use tokio::io::AsyncWriteExt;

        let res = self.fetch(container, blob, false, None, if_match).await?;
        if res.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(BlobChanged.into());
        }
        if !res.status().is_success() {
            bail!("GET {}/{blob}: {}", container.as_str(), res.status());
        }
        let mut file = tokio::fs::File::create(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;
        let mut written = 0u64;
        let mut body = res;
        while let Some(chunk) = body
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)
            .context("reading blob body")?
        {
            file.write_all(&chunk).await.context("writing download")?;
            written += chunk.len() as u64;
        }
        file.flush().await?;
        Ok(written)
    }

    /// Size of a blob, or `None` if it doesn't exist.
    pub async fn blob_size(&self, container: Container, blob: &str) -> anyhow::Result<Option<u64>> {
        Ok(self.blob_props(container, blob).await?.map(|p| p.size))
    }

    /// Size and ETag of a blob, or `None` if it doesn't exist.
    pub async fn blob_props(
        &self,
        container: Container,
        blob: &str,
    ) -> anyhow::Result<Option<BlobProps>> {
        let url = self
            .sas_url(
                container,
                blob,
                Access::Read,
                Duration::from_secs(300),
                &Overrides::default(),
            )
            .await?;
        let res = self
            .http
            .head(url)
            .header("x-ms-version", VERSION)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("HEAD blob")?;
        match res.status() {
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => {
                let header = |name| res.headers().get(name).and_then(|v| v.to_str().ok());
                Ok(Some(BlobProps {
                    size: header(reqwest::header::CONTENT_LENGTH)
                        .and_then(|v| v.parse().ok())
                        .context("HEAD blob: no Content-Length")?,
                    etag: header(reqwest::header::ETAG)
                        .context("HEAD blob: no ETag")?
                        .to_owned(),
                }))
            }
            s => bail!("HEAD {}/{blob}: {s}", container.as_str()),
        }
    }

    /// Uploads a local file as a block blob (single Put Blob, up to 5000 MiB).
    pub async fn upload_file(
        &self,
        container: Container,
        blob: &str,
        path: &Path,
        content_type: &str,
    ) -> anyhow::Result<()> {
        let url = self
            .sas_url(
                container,
                blob,
                Access::Write,
                Duration::from_secs(3600),
                &Overrides::default(),
            )
            .await?;
        // A pooled connection the server has already closed as idle fails as the body goes
        // out (broken pipe, reset). Put Blob replaces the whole blob, so it's safe to send
        // it again, once, on a fresh connection.
        let mut retried = false;
        let res = loop {
            let file = tokio::fs::File::open(path)
                .await
                .with_context(|| format!("opening {}", path.display()))?;
            let len = file.metadata().await?.len();
            let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                file,
                1 << 20,
            ));
            let sent = self
                .http
                .put(url.clone())
                .header("x-ms-version", VERSION)
                .header("x-ms-blob-type", "BlockBlob")
                .header("x-ms-blob-content-type", content_type)
                .header(reqwest::header::CONTENT_LENGTH, len)
                .body(body)
                .send()
                .await;
            match sent {
                Ok(res) => break res,
                Err(e) if !retried && dropped_connection(&e) => {
                    tracing::debug!(error = %e.without_url(), "Put Blob: connection dropped; sending again");
                    retried = true;
                }
                Err(e) => return Err(e.without_url()).context("Put Blob"),
            }
        };
        if !res.status().is_success() {
            bail!(
                "Put Blob {}/{blob}: {} {}",
                container.as_str(),
                res.status(),
                res.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }

    /// Deletes a blob; already gone is fine.
    pub async fn delete(&self, container: Container, blob: &str) -> anyhow::Result<()> {
        let url = self
            .sas_url(
                container,
                blob,
                Access::Delete,
                Duration::from_secs(300),
                &Overrides::default(),
            )
            .await?;
        let res = self
            .http
            .delete(url)
            .header("x-ms-version", VERSION)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Delete Blob")?;
        if res.status().is_success() || res.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            bail!(
                "Delete Blob {}/{blob}: {}",
                container.as_str(),
                res.status()
            )
        }
    }

    /// Azurite only: creates the containers and sets browser CORS, which `infra/azure`
    /// does in Azure. A no-op with a user delegation key.
    pub async fn prepare_local(&self, cors_origins: &[&str]) -> anyhow::Result<()> {
        let Signer::SharedKey(key) = &self.signer else {
            return Ok(());
        };
        let expiry = fmt_time(Utc::now() + Duration::from_secs(300));
        let to_sign = format!(
            "{}\nrwdlac\nb\nsco\n\n{expiry}\n\n\n{VERSION}\n\n",
            self.account
        );
        let sas = [
            ("sv", VERSION),
            ("ss", "b"),
            ("srt", "sco"),
            ("sp", "rwdlac"),
            ("se", &expiry),
            ("sig", &sign(key, &to_sign)),
        ];

        for container in Container::ALL {
            let mut url = self.endpoint.clone();
            url.path_segments_mut()
                .expect("http(s) URL")
                .pop_if_empty()
                .push(container.as_str());
            url.query_pairs_mut()
                .append_pair("restype", "container")
                .extend_pairs(sas);
            let res = self
                .http
                .put(url)
                .header("x-ms-version", VERSION)
                .header(reqwest::header::CONTENT_LENGTH, 0)
                .send()
                .await
                .map_err(reqwest::Error::without_url)?;
            if !(res.status().is_success() || res.status() == reqwest::StatusCode::CONFLICT) {
                bail!(
                    "creating container {}: {}",
                    container.as_str(),
                    res.status()
                );
            }
        }

        let rules: String = cors_origins
            .iter()
            .map(|o| {
                format!(
                    "<CorsRule><AllowedOrigins>{o}</AllowedOrigins>\
                     <AllowedMethods>PUT,GET,HEAD,OPTIONS</AllowedMethods>\
                     <AllowedHeaders>*</AllowedHeaders><ExposedHeaders>*</ExposedHeaders>\
                     <MaxAgeInSeconds>3600</MaxAgeInSeconds></CorsRule>"
                )
            })
            .collect();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><StorageServiceProperties>\
             <Cors>{rules}</Cors></StorageServiceProperties>"
        );
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("restype", "service")
            .append_pair("comp", "properties")
            .extend_pairs(sas);
        let res = self
            .http
            .put(url)
            .header("x-ms-version", VERSION)
            .body(body)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        if !res.status().is_success() {
            bail!("setting Azurite CORS: {}", res.status());
        }
        Ok(())
    }

    async fn delegation_key(&self) -> anyhow::Result<DelegationKey> {
        let Signer::UserDelegation(delegation) = &self.signer else {
            unreachable!("only called for user delegation signing");
        };
        let UserDelegation { credential, key } = delegation.as_ref();
        let fresh = |k: &DelegationKey| {
            (k.expires_at - Utc::now()).to_std().unwrap_or_default() > DELEGATION_KEY_LIFETIME / 2
        };
        if let Some(k) = key.read().await.as_ref().filter(|k| fresh(k)) {
            return Ok(k.clone());
        }

        let mut slot = key.write().await;
        if let Some(k) = slot.as_ref().filter(|k| fresh(k)) {
            return Ok(k.clone());
        }
        let token = credential
            .token(STORAGE_RESOURCE)
            .await
            .context("getting an Entra token for Blob Storage")?;
        let now = Utc::now();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><KeyInfo><Start>{}</Start><Expiry>{}</Expiry></KeyInfo>",
            fmt_time(now - START_SKEW),
            fmt_time(now + DELEGATION_KEY_LIFETIME)
        );
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("restype", "service")
            .append_pair("comp", "userdelegationkey");
        let res = self
            .http
            .post(url)
            .bearer_auth(&token.secret)
            .header("x-ms-version", VERSION)
            .body(body)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("requesting a user delegation key")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("user delegation key: {status} {text}");
        }
        let fresh_key = parse_delegation_key(&text)?;
        tracing::debug!(expires = %fresh_key.expiry, "fetched a user delegation key");
        *slot = Some(fresh_key.clone());
        Ok(fresh_key)
    }
}

fn parse_delegation_key(xml: &str) -> anyhow::Result<DelegationKey> {
    let tag = |name: &str| -> anyhow::Result<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = xml
            .find(&open)
            .with_context(|| format!("no <{name}> in {xml}"))?
            + open.len();
        let end = xml[start..]
            .find(&close)
            .with_context(|| format!("unterminated <{name}>"))?;
        Ok(xml[start..start + end].to_owned())
    };
    let expiry = tag("SignedExpiry")?;
    Ok(DelegationKey {
        oid: tag("SignedOid")?,
        tid: tag("SignedTid")?,
        start: tag("SignedStart")?,
        expires_at: DateTime::parse_from_rfc3339(&expiry)
            .context("parsing SignedExpiry")?
            .with_timezone(&Utc),
        expiry,
        service: tag("SignedService")?,
        version: tag("SignedVersion")?,
        value: B64
            .decode(tag("Value")?)
            .context("decoding the delegation key")?,
    })
}

fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn sign(key: &[u8], to_sign: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(to_sign.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Whether a request failed because the connection went away while it was being sent
/// (rather than timing out, or the server answering).
fn dropped_connection(e: &reqwest::Error) -> bool {
    (e.is_request() || e.is_connect()) && !e.is_timeout()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{IntoResponse, Response},
    };

    use super::*;

    #[test]
    fn parses_delegation_key_response() {
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><UserDelegationKey>\
            <SignedOid>oid</SignedOid><SignedTid>tid</SignedTid>\
            <SignedStart>2026-10-01T00:00:00Z</SignedStart>\
            <SignedExpiry>2026-10-03T00:00:00Z</SignedExpiry>\
            <SignedService>b</SignedService><SignedVersion>2024-11-04</SignedVersion>\
            <Value>c2VjcmV0</Value></UserDelegationKey>";
        let key = parse_delegation_key(xml).unwrap();
        assert_eq!(key.oid, "oid");
        assert_eq!(key.value, b"secret".to_vec());
        assert_eq!(key.expires_at.to_rfc3339(), "2026-10-03T00:00:00+00:00");
    }

    /// Azurite's well-known development account (public, not a secret).
    pub(crate) fn azurite() -> Option<Storage> {
        std::env::var_os("CLIPOS_AZURITE")?;
        Some(
            Storage::new(StorageConfig {
                account: "devstoreaccount1".into(),
                blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
                account_key: Some(
                    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                        .into(),
                ),
            })
            .unwrap(),
        )
    }

    /// Runs against Azurite when `CLIPOS_AZURITE` is set (CI and `make test`).
    #[tokio::test]
    async fn sas_round_trip_on_azurite() {
        let Some(storage) = azurite() else {
            eprintln!("CLIPOS_AZURITE not set; skipping");
            return;
        };
        storage
            .prepare_local(&["http://localhost:5173"])
            .await
            .unwrap();

        let blob = format!("test/{}/clip one.mp4", uuid::Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.bin");
        std::fs::write(&path, vec![7u8; 300_000]).unwrap();

        assert_eq!(
            storage
                .blob_size(Container::Originals, &blob)
                .await
                .unwrap(),
            None
        );
        storage
            .upload_file(Container::Originals, &blob, &path, "video/mp4")
            .await
            .unwrap();
        assert_eq!(
            storage
                .blob_size(Container::Originals, &blob)
                .await
                .unwrap(),
            Some(300_000)
        );

        // A download with `If-Match` reads the blob only while it's still that version.
        let etag = storage
            .blob_props(Container::Originals, &blob)
            .await
            .unwrap()
            .unwrap()
            .etag;
        let copy = dir.path().join("copy.bin");
        assert_eq!(
            storage
                .download_to_file(Container::Originals, &blob, &copy, Some(&etag))
                .await
                .unwrap(),
            300_000
        );
        let err = storage
            .download_to_file(Container::Originals, &blob, &copy, Some("\"0x1\""))
            .await
            .unwrap_err();
        assert!(err.is::<BlobChanged>(), "{err:#}");

        let url = storage
            .sas_url(
                Container::Originals,
                &blob,
                Access::Read,
                Duration::from_secs(60),
                &Overrides {
                    content_disposition: Some("attachment; filename=\"clip.mp4\"".into()),
                    content_type: None,
                },
            )
            .await
            .unwrap();
        let res = reqwest::get(url.clone()).await.unwrap();
        assert!(res.status().is_success(), "{}", res.status());
        assert_eq!(
            res.headers()["content-disposition"],
            "attachment; filename=\"clip.mp4\""
        );
        assert_eq!(res.bytes().await.unwrap().len(), 300_000);

        // A read SAS can't write, and a SAS for one blob can't read another.
        let mut other = url.clone();
        other.set_path(&url.path().replace("clip%20one", "other"));
        assert_eq!(reqwest::get(other).await.unwrap().status(), 403);
        let put = reqwest::Client::new()
            .put(url)
            .header("x-ms-blob-type", "BlockBlob")
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), 403);

        storage.delete(Container::Originals, &blob).await.unwrap();
        storage.delete(Container::Originals, &blob).await.unwrap();
        assert_eq!(
            storage
                .blob_size(Container::Originals, &blob)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn errors_leave_out_the_sas() {
        let storage = Storage::new(StorageConfig {
            account: "devstoreaccount1".into(),
            // Nothing listens here, so every request fails to connect.
            blob_endpoint: Some("http://127.0.0.1:9/devstoreaccount1".into()),
            account_key: Some(B64.encode("k")),
        })
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"x").unwrap();
        let errors = [
            storage
                .blob_size(Container::Originals, "clip/a.mp4")
                .await
                .unwrap_err(),
            storage
                .fetch(Container::Playback, "a.mp4", false, Some("bytes=0-1"), None)
                .await
                .unwrap_err(),
            storage
                .download_to_file(Container::Originals, "clip/a.mp4", &path, None)
                .await
                .unwrap_err(),
            storage
                .upload_file(Container::Playback, "a.mp4", &path, "video/mp4")
                .await
                .unwrap_err(),
            storage
                .delete(Container::Posters, "a.jpg")
                .await
                .unwrap_err(),
            storage.prepare_local(&[]).await.unwrap_err(),
        ];
        for e in errors {
            for text in [format!("{e:#}"), format!("{e:?}")] {
                assert!(!text.contains("sig="), "{text}");
                assert!(!text.contains("127.0.0.1:9"), "{text}");
            }
        }
    }

    #[test]
    fn blob_urls_encode_names() {
        let storage = Storage::new(StorageConfig {
            account: "acct".into(),
            blob_endpoint: None,
            account_key: Some(B64.encode("k")),
        })
        .unwrap();
        assert_eq!(
            storage
                .blob_url(Container::Originals, "abc/my clip #1.mp4")
                .as_str(),
            "https://acct.blob.core.windows.net/originals/abc/my%20clip%20%231.mp4"
        );
    }

    #[test]
    fn bad_config_is_an_error() {
        let err = Storage::new(StorageConfig {
            account: "acct".into(),
            blob_endpoint: None,
            account_key: Some("not base64!".into()),
        })
        .err()
        .unwrap();
        assert_eq!(format!("{err}"), "decoding the storage account key");

        let err = Storage::new(StorageConfig {
            account: "acct".into(),
            blob_endpoint: Some("not a url".into()),
            account_key: Some(B64.encode("k")),
        })
        .err()
        .unwrap();
        assert_eq!(format!("{err}"), "parsing the blob endpoint");
    }

    #[test]
    fn without_a_key_sas_are_signed_with_a_delegation_key() {
        let storage = Storage::new(StorageConfig {
            account: "acct".into(),
            blob_endpoint: None,
            account_key: None,
        })
        .unwrap();
        assert!(matches!(storage.signer, Signer::UserDelegation(_)));
        assert_eq!(
            storage.endpoint.as_str(),
            "https://acct.blob.core.windows.net/"
        );
    }

    #[test]
    fn containers_have_names() {
        for c in Container::ALL {
            assert_eq!(Container::from_name(c.as_str()), Some(c));
        }
        assert_eq!(Container::from_name("thumbnails"), None);
    }

    /// The query parameters of `url`, in order.
    fn query(url: &Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn param<'a>(q: &'a [(String, String)], name: &str) -> Option<&'a str> {
        q.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    #[tokio::test]
    async fn shared_key_sas_on_https() {
        let storage = Storage::new(StorageConfig {
            account: "acct".into(),
            blob_endpoint: None,
            account_key: Some(B64.encode("key")),
        })
        .unwrap();
        assert_eq!(storage.origin(), "https://acct.blob.core.windows.net");

        let url = storage
            .sas_url(
                Container::Playback,
                "a/b.mp4",
                Access::Write,
                Duration::from_secs(600),
                &Overrides {
                    content_disposition: None,
                    content_type: Some("video/mp4".into()),
                },
            )
            .await
            .unwrap();
        let q = query(&url);
        let names: Vec<&str> = q.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            ["st", "se", "sv", "sr", "sp", "spr", "rsct", "sig"],
            "https only, and no rscd when it isn't overridden"
        );
        assert_eq!(param(&q, "sp"), Some("cw"));
        assert_eq!(param(&q, "spr"), Some("https"));

        // Signed over Azure's service SAS fields, in Azure's order.
        let (st, se) = (param(&q, "st").unwrap(), param(&q, "se").unwrap());
        let to_sign = [
            "cw",                          // sp
            st,                            // st
            se,                            // se
            "/blob/acct/playback/a/b.mp4", // canonicalized resource
            "",                            // si
            "",                            // sip
            "https",                       // spr
            VERSION,                       // sv
            "b",                           // sr
            "",                            // snapshot
            "",                            // ses
            "",                            // rscc
            "",                            // rscd
            "",                            // rsce
            "",                            // rscl
            "video/mp4",                   // rsct
        ]
        .join("\n");
        assert_eq!(param(&q, "sig"), Some(sign(b"key", &to_sign).as_str()));
        let ttl =
            DateTime::parse_from_rfc3339(se).unwrap() - DateTime::parse_from_rfc3339(st).unwrap();
        assert_eq!(
            ttl.num_seconds(),
            (START_SKEW + Duration::from_secs(600)).as_secs() as i64
        );
    }

    /// What a mock server saw: method, path and query, and headers.
    type Seen = Arc<Mutex<Vec<(Method, String, HeaderMap)>>>;

    /// A stand-in for Blob Storage (and the managed-identity endpoint) on 127.0.0.1 that
    /// answers every request with `respond`. Returns its base URL and what it saw.
    async fn mock(
        respond: impl Fn(&Method, &Uri, &HeaderMap) -> Response + Send + Sync + 'static,
    ) -> (String, Seen) {
        let respond = Arc::new(respond);
        let seen = Seen::default();
        let log = seen.clone();
        let app =
            axum::Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
                let respond = respond.clone();
                let log = log.clone();
                async move {
                    let res = respond(&method, &uri, &headers);
                    log.lock().unwrap().push((method, uri.to_string(), headers));
                    res
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    fn status(code: u16, body: &str) -> Response {
        (StatusCode::from_u16(code).unwrap(), body.to_owned()).into_response()
    }

    /// Storage signing with Azurite's key, against a mock at `base`.
    fn shared_key(base: &str) -> Storage {
        Storage::new(StorageConfig {
            account: "devstoreaccount1".into(),
            blob_endpoint: Some(format!("{base}/devstoreaccount1")),
            account_key: Some(B64.encode("k")),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn fetch_passes_range_and_if_match_through() {
        let (base, seen) = mock(|_, _, _| status(206, "0123")).await;
        let storage = shared_key(&base);

        let res = storage
            .fetch(
                Container::Playback,
                "c/v.mp4",
                false,
                Some("bytes=0-3"),
                Some("\"0x8D\""),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 206);
        storage
            .fetch(Container::Posters, "c/p.jpg", true, None, None)
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        let (method, uri, headers) = &seen[0];
        assert_eq!(method, Method::GET);
        assert!(
            uri.starts_with("/devstoreaccount1/playback/c/v.mp4?"),
            "{uri}"
        );
        assert!(uri.contains("sp=r&"), "a read SAS: {uri}");
        assert_eq!(headers["range"], "bytes=0-3");
        assert_eq!(headers["if-match"], "\"0x8D\"");
        assert_eq!(headers["x-ms-version"], VERSION);
        let (method, _, headers) = &seen[1];
        assert_eq!(method, Method::HEAD);
        assert!(headers.get("range").is_none());
        assert!(headers.get("if-match").is_none());
    }

    #[tokio::test]
    async fn error_statuses_are_errors() {
        let (base, _) = mock(|method, uri, _| match (method.as_str(), uri.path()) {
            ("HEAD", "/devstoreaccount1/originals/no-etag") => {
                (StatusCode::OK, [("content-length", "5")], "").into_response()
            }
            ("PUT", _) => status(403, "AuthorizationFailure"),
            _ => status(500, "oops"),
        })
        .await;
        let storage = shared_key(&base);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        fn err<T>(r: anyhow::Result<T>) -> String {
            format!("{:#}", r.err().unwrap())
        }

        assert_eq!(
            err(storage.blob_props(Container::Originals, "a").await),
            "HEAD originals/a: 500 Internal Server Error"
        );
        assert_eq!(
            err(storage.blob_props(Container::Originals, "no-etag").await),
            "HEAD blob: no ETag"
        );
        assert_eq!(
            err(storage
                .download_to_file(Container::Originals, "a", &file, None)
                .await),
            "GET originals/a: 500 Internal Server Error"
        );
        assert_eq!(
            err(storage
                .upload_file(Container::Playback, "a.mp4", &file, "video/mp4")
                .await),
            "Put Blob playback/a.mp4: 403 Forbidden AuthorizationFailure"
        );
        assert_eq!(
            err(storage.delete(Container::Posters, "a.jpg").await),
            "Delete Blob posters/a.jpg: 500 Internal Server Error"
        );
        assert_eq!(
            err(storage.prepare_local(&[]).await),
            "creating container originals: 403 Forbidden"
        );

        let missing = dir.path().join("missing");
        assert!(
            err(storage
                .upload_file(Container::Playback, "a.mp4", &missing, "video/mp4")
                .await)
            .starts_with(&format!("opening {}", missing.display()))
        );
    }

    #[tokio::test]
    async fn a_download_needs_somewhere_to_write() {
        let (base, _) = mock(|_, _, _| status(200, "data")).await;
        let path = Path::new("/nonexistent-clipos-dir/f");
        let err = shared_key(&base)
            .download_to_file(Container::Originals, "a", path, None)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("creating /nonexistent-clipos-dir/f"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn prepare_local_sets_cors_for_the_origins() {
        // Containers that already exist are fine; the CORS rules are checked.
        let (base, seen) = mock(|_, uri, _| {
            if uri.query().is_some_and(|q| q.contains("restype=container")) {
                status(409, "ContainerAlreadyExists")
            } else {
                status(400, "InvalidXmlDocument")
            }
        })
        .await;
        let err = shared_key(&base)
            .prepare_local(&["http://localhost:5173", "http://127.0.0.1:4173"])
            .await
            .unwrap_err();
        assert_eq!(format!("{err:#}"), "setting Azurite CORS: 400 Bad Request");

        let seen = seen.lock().unwrap();
        let containers: Vec<&str> = seen[..4]
            .iter()
            .map(|(_, uri, _)| uri.split('?').next().unwrap())
            .collect();
        assert_eq!(
            containers,
            [
                "/devstoreaccount1/originals",
                "/devstoreaccount1/playback",
                "/devstoreaccount1/posters",
                "/devstoreaccount1/models"
            ]
        );
        let (method, uri, _) = &seen[4];
        assert_eq!(method, Method::PUT);
        assert!(uri.contains("restype=service&comp=properties"), "{uri}");
        assert!(uri.contains("sp=rwdlac"), "{uri}");
    }

    const DELEGATION_TOKEN: &str = "entra-token-for-storage";

    /// A user delegation key response for a key that expires at `expiry`.
    fn delegation_key_xml(expiry: DateTime<Utc>) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><UserDelegationKey>\
             <SignedOid>the-oid</SignedOid><SignedTid>the-tid</SignedTid>\
             <SignedStart>2026-10-01T00:00:00Z</SignedStart>\
             <SignedExpiry>{}</SignedExpiry>\
             <SignedService>b</SignedService><SignedVersion>{VERSION}</SignedVersion>\
             <Value>{}</Value></UserDelegationKey>",
            fmt_time(expiry),
            B64.encode("delegation-secret")
        )
    }

    /// A storage account and identity endpoint on one mock: tokens from `/msi/token`, the
    /// delegation key from `respond_key`, anything else 404.
    async fn delegation(
        respond_key: impl Fn(&HeaderMap) -> Response + Send + Sync + 'static,
    ) -> (Storage, Seen) {
        let (base, seen) = mock(move |method, uri, headers| {
            if uri.path() == "/msi/token" {
                axum::Json(serde_json::json!({
                    "access_token": DELEGATION_TOKEN,
                    "expires_on": "1900000000",
                }))
                .into_response()
            } else if method == Method::POST
                && uri.query() == Some("restype=service&comp=userdelegationkey")
            {
                respond_key(headers)
            } else {
                status(404, "")
            }
        })
        .await;
        let storage = Storage::with_credential(
            StorageConfig {
                account: "acct".into(),
                blob_endpoint: Some(format!("{base}/acct")),
                account_key: None,
            },
            || Credential::app_service(format!("{base}/msi/token"), "header"),
        )
        .unwrap();
        (storage, seen)
    }

    fn key_requests(seen: &Seen) -> usize {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|(_, uri, _)| uri.contains("userdelegationkey"))
            .count()
    }

    #[tokio::test]
    async fn user_delegation_sas() {
        let key_expiry = Utc::now() + chrono::Duration::days(2);
        let (storage, seen) = delegation(move |headers| {
            if headers["authorization"] != format!("Bearer {DELEGATION_TOKEN}") {
                return status(403, "");
            }
            delegation_key_xml(key_expiry).into_response()
        })
        .await;

        let url = storage
            .sas_url(
                Container::Originals,
                "c/clip.mp4",
                Access::Read,
                Duration::from_secs(3600),
                &Overrides {
                    content_disposition: Some("attachment".into()),
                    content_type: Some("video/mp4".into()),
                },
            )
            .await
            .unwrap();
        let q = query(&url);
        let names: Vec<&str> = q.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "st", "se", "skoid", "sktid", "skt", "ske", "sks", "skv", "sv", "sr", "sp", "rscd",
                "rsct", "sig"
            ],
            "no spr over http"
        );
        assert_eq!(param(&q, "skoid"), Some("the-oid"));
        assert_eq!(param(&q, "ske"), Some(fmt_time(key_expiry).as_str()));

        // Signed with the delegation key over Azure's user delegation SAS fields.
        let (st, se) = (param(&q, "st").unwrap(), param(&q, "se").unwrap());
        let ske = fmt_time(key_expiry);
        let to_sign = [
            "r",                               // sp
            st,                                // st
            se,                                // se
            "/blob/acct/originals/c/clip.mp4", // canonicalized resource
            "the-oid",                         // skoid
            "the-tid",                         // sktid
            "2026-10-01T00:00:00Z",            // skt
            &ske,                              // ske
            "b",                               // sks
            VERSION,                           // skv
            "",                                // saoid
            "",                                // suoid
            "",                                // scid
            "",                                // sip
            "",                                // spr
            VERSION,                           // sv
            "b",                               // sr
            "",                                // snapshot
            "",                                // ses
            "",                                // rscc
            "attachment",                      // rscd
            "",                                // rsce
            "",                                // rscl
            "video/mp4",                       // rsct
        ]
        .join("\n");
        assert_eq!(
            param(&q, "sig"),
            Some(sign(b"delegation-secret", &to_sign).as_str())
        );

        // The key is kept while it has more than half its life left.
        storage
            .sas_url(
                Container::Posters,
                "p.jpg",
                Access::Read,
                Duration::from_secs(60),
                &Overrides::default(),
            )
            .await
            .unwrap();
        assert_eq!(key_requests(&seen), 1);

        // A SAS can't outlive the key that signed it.
        let url = storage
            .sas_url(
                Container::Originals,
                "c/clip.mp4",
                Access::Delete,
                Duration::from_secs(7 * 24 * 3600),
                &Overrides::default(),
            )
            .await
            .unwrap();
        assert_eq!(param(&query(&url), "se"), Some(ske.as_str()));

        // Nothing to prepare outside Azurite.
        let before = seen.lock().unwrap().len();
        storage
            .prepare_local(&["http://localhost:5173"])
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn an_aging_delegation_key_is_replaced() {
        // Half its life is gone: replaced on next use.
        let (storage, seen) = delegation(|_| {
            delegation_key_xml(Utc::now() + chrono::Duration::hours(12)).into_response()
        })
        .await;
        for _ in 0..2 {
            storage
                .sas_url(
                    Container::Playback,
                    "v.mp4",
                    Access::Read,
                    Duration::from_secs(60),
                    &Overrides::default(),
                )
                .await
                .unwrap();
        }
        assert_eq!(key_requests(&seen), 2);
    }

    #[tokio::test]
    async fn delegation_key_failures_are_errors() {
        let sas = |storage: Storage| async move {
            let err = storage
                .sas_url(
                    Container::Originals,
                    "a",
                    Access::Read,
                    Duration::from_secs(60),
                    &Overrides::default(),
                )
                .await
                .unwrap_err();
            format!("{err:#}")
        };

        let (storage, _) = delegation(|_| status(403, "AuthorizationPermissionMismatch")).await;
        assert_eq!(
            sas(storage).await,
            "user delegation key: 403 Forbidden AuthorizationPermissionMismatch"
        );

        let (storage, _) = delegation(|_| status(200, "<Error/>")).await;
        assert_eq!(sas(storage).await, "no <SignedExpiry> in <Error/>");

        // No identity endpoint: no token, so no key.
        let storage = Storage::with_credential(
            StorageConfig {
                account: "acct".into(),
                blob_endpoint: Some("http://127.0.0.1:9/acct".into()),
                account_key: None,
            },
            || Credential::app_service("http://127.0.0.1:9/msi/token", "header"),
        )
        .unwrap();
        assert!(
            sas(storage)
                .await
                .starts_with("getting an Entra token for Blob Storage"),
        );

        // A token, but Blob Storage doesn't answer.
        let (base, _) = mock(|_, _, _| {
            axum::Json(serde_json::json!({ "access_token": "t", "expires_on": "1900000000" }))
                .into_response()
        })
        .await;
        let storage = Storage::with_credential(
            StorageConfig {
                account: "acct".into(),
                blob_endpoint: Some("http://127.0.0.1:9/acct".into()),
                account_key: None,
            },
            move || Credential::app_service(format!("{base}/msi/token"), "header"),
        )
        .unwrap();
        let err = sas(storage).await;
        assert!(err.starts_with("requesting a user delegation key"), "{err}");
        assert!(!err.contains("127.0.0.1:9"), "{err}");
    }

    #[test]
    fn malformed_delegation_keys_are_errors() {
        let valid = delegation_key_xml(Utc::now());
        let err = |xml: &str| format!("{:#}", parse_delegation_key(xml).unwrap_err());

        assert_eq!(
            err(&valid.replace("</SignedOid>", "")),
            "unterminated <SignedOid>"
        );
        assert!(
            err(&valid.replace(&fmt_time(Utc::now())[..4], "soon"))
                .starts_with("parsing SignedExpiry"),
        );
        assert!(
            err(&valid.replace(&B64.encode("delegation-secret"), "not base64!"))
                .starts_with("decoding the delegation key"),
        );
    }

    /// Runs against Azurite when `CLIPOS_AZURITE` is set (CI and `make test`).
    #[tokio::test]
    async fn ranged_reads_on_azurite() {
        let Some(storage) = azurite() else {
            eprintln!("CLIPOS_AZURITE not set; skipping");
            return;
        };
        storage.prepare_local(&[]).await.unwrap();
        let blob = format!("test/{}/range.bin", uuid::Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.bin");
        std::fs::write(&path, (0..=255u8).collect::<Vec<_>>()).unwrap();
        storage
            .upload_file(
                Container::Playback,
                &blob,
                &path,
                "application/octet-stream",
            )
            .await
            .unwrap();
        let props = storage
            .blob_props(Container::Playback, &blob)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(props.size, 256);

        let res = storage
            .fetch(
                Container::Playback,
                &blob,
                false,
                Some("bytes=10-13"),
                Some(&props.etag),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 206);
        assert_eq!(res.headers()["content-range"], "bytes 10-13/256");
        assert_eq!(res.bytes().await.unwrap().as_ref(), [10, 11, 12, 13]);

        let res = storage
            .fetch(Container::Playback, &blob, true, None, None)
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["content-length"], "256");
        assert_eq!(res.headers()["content-type"], "application/octet-stream");

        // A stale ETag is a 412 for the caller to handle.
        let res = storage
            .fetch(Container::Playback, &blob, false, None, Some("\"0x1\""))
            .await
            .unwrap();
        assert_eq!(res.status(), 412);

        storage.delete(Container::Playback, &blob).await.unwrap();
    }

    /// A server on 127.0.0.1 that closes the first `drops` connections as soon as it has
    /// read a request's headers (as a server does to a pooled connection it had closed as
    /// idle) and answers 201 on the ones after. Returns its base URL and how many
    /// connections it took.
    async fn dropping(drops: usize) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let taken = Arc::new(AtomicUsize::new(0));
        let count = taken.clone();
        tokio::spawn(async move {
            loop {
                let (mut conn, _) = listener.accept().await.unwrap();
                let n = count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(k) => req.extend_from_slice(&buf[..k]),
                        }
                    }
                    if n < drops {
                        return; // dropped: the connection closes mid-request
                    }
                    let head_end = req.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let head = String::from_utf8_lossy(&req[..head_end]).to_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    let mut got = req.len() - head_end;
                    while got < len {
                        match conn.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(k) => got += k,
                        }
                    }
                    let _ = conn
                        .write_all(b"HTTP/1.1 201 Created\r\ncontent-length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        (format!("http://{addr}"), taken)
    }

    #[tokio::test]
    async fn an_upload_is_sent_again_once_when_its_connection_drops() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, vec![7u8; 256 * 1024]).unwrap();

        // Dropped once: the second try, on a fresh connection, uploads it.
        let (base, taken) = dropping(1).await;
        shared_key(&base)
            .upload_file(Container::Playback, "a.mp4", &file, "video/mp4")
            .await
            .unwrap();
        assert_eq!(taken.load(Ordering::SeqCst), 2);

        // Dropped every time: one retry only, then the error.
        let (base, taken) = dropping(usize::MAX).await;
        let err = shared_key(&base)
            .upload_file(Container::Playback, "a.mp4", &file, "video/mp4")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").starts_with("Put Blob: "), "{err:#}");
        assert!(!format!("{err:#}").contains("sig="), "{err:#}");
        assert_eq!(taken.load(Ordering::SeqCst), 2);
    }
}
