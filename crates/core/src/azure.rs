//! Microsoft Entra access tokens for Azure resources (Postgres now, Blob Storage later).
//!
//! In App Service the token comes from the managed-identity endpoint the platform injects
//! (`IDENTITY_ENDPOINT` + `IDENTITY_HEADER`). Anywhere else it comes from the Azure CLI
//! login, so the same binary can reach the real server from a laptop.

use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use serde::Deserialize;

/// Token audience for Azure Database for PostgreSQL.
pub const POSTGRES_RESOURCE: &str = "https://ossrdbms-aad.database.windows.net";

#[derive(Clone)]
pub struct AccessToken {
    pub secret: String,
    pub expires_at: SystemTime,
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("secret", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum Credential {
    /// App Service managed identity (system-assigned).
    AppService {
        endpoint: String,
        header: String,
        http: reqwest::Client,
    },
    /// `az account get-access-token` with whoever is logged in to the Azure CLI.
    AzureCli,
}

impl Credential {
    /// The managed identity when running in App Service, otherwise the Azure CLI.
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var("IDENTITY_ENDPOINT").ok(),
            std::env::var("IDENTITY_HEADER").ok(),
        )
    }

    /// `from_env`, given `IDENTITY_ENDPOINT` and `IDENTITY_HEADER`.
    fn from_vars(endpoint: Option<String>, header: Option<String>) -> Self {
        match (endpoint, header) {
            (Some(endpoint), Some(header)) => Self::app_service(endpoint, header),
            _ => Self::AzureCli,
        }
    }

    pub fn app_service(endpoint: impl Into<String>, header: impl Into<String>) -> Self {
        Self::AppService {
            endpoint: endpoint.into(),
            header: header.into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("static reqwest client config"),
        }
    }

    pub async fn token(&self, resource: &str) -> anyhow::Result<AccessToken> {
        let raw: RawToken = match self {
            Self::AppService {
                endpoint,
                header,
                http,
            } => http
                .get(endpoint)
                .query(&[("resource", resource), ("api-version", "2019-08-01")])
                .header("X-IDENTITY-HEADER", header)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .context("requesting a managed identity token")?
                .json()
                .await
                .context("decoding the managed identity token")?,
            Self::AzureCli => cli_token("az", resource).await?,
        };
        raw.into_token()
    }
}

/// `az account get-access-token` for `resource`, with `az` at `program`.
async fn cli_token(program: &str, resource: &str) -> anyhow::Result<RawToken> {
    let out = tokio::process::Command::new(program)
        .args(["account", "get-access-token", "--output", "json"])
        .args(["--resource", resource])
        .output()
        .await
        .context("running `az` (is the Azure CLI installed?)")?;
    if !out.status.success() {
        bail!(
            "az account get-access-token failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("decoding `az` token output")
}

/// Both sources share this shape: the managed-identity endpoint uses `access_token`
/// with `expires_on` as a string, the CLI `accessToken` with a number.
#[derive(Deserialize)]
struct RawToken {
    #[serde(alias = "accessToken")]
    access_token: String,
    expires_on: EpochSecs,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EpochSecs {
    Number(u64),
    Text(String),
}

impl RawToken {
    fn into_token(self) -> anyhow::Result<AccessToken> {
        let secs = match self.expires_on {
            EpochSecs::Number(n) => n,
            EpochSecs::Text(s) => s
                .parse()
                .with_context(|| format!("invalid expires_on {s:?}"))?,
        };
        Ok(AccessToken {
            secret: self.access_token,
            expires_at: UNIX_EPOCH + Duration::from_secs(secs),
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        extract::Query,
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use serde_json::{Value, json};
    use std::collections::HashMap;

    use super::*;

    async fn identity_endpoint(
        headers: HeaderMap,
        Query(query): Query<HashMap<String, String>>,
    ) -> Result<Json<Value>, StatusCode> {
        if headers.get("x-identity-header").map(|v| v.as_bytes()) != Some(b"secret-header") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(json!({
            "access_token": format!("token-for-{}", query["resource"]),
            "expires_on": "1900000000",
            "resource": query["resource"],
            "token_type": "Bearer",
        })))
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}/msi/token")
    }

    #[tokio::test]
    async fn app_service_identity_token() {
        let endpoint = serve(Router::new().route("/msi/token", get(identity_endpoint))).await;

        let token = Credential::app_service(&endpoint, "secret-header")
            .token(POSTGRES_RESOURCE)
            .await
            .unwrap();

        assert_eq!(token.secret, format!("token-for-{POSTGRES_RESOURCE}"));
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_900_000_000)
        );
    }

    #[tokio::test]
    async fn app_service_rejection_is_an_error() {
        let endpoint = serve(Router::new().route("/msi/token", get(identity_endpoint))).await;

        let err = Credential::app_service(&endpoint, "wrong")
            .token(POSTGRES_RESOURCE)
            .await
            .unwrap_err();

        assert!(format!("{err:#}").contains("401"), "{err:#}");
    }

    #[tokio::test]
    async fn app_service_garbage_is_an_error() {
        let endpoint = serve(Router::new().route(
            "/msi/token",
            get(|| async { Json(json!({ "access_token": "t", "expires_on": "soon" })) }),
        ))
        .await;
        let err = Credential::app_service(&endpoint, "h")
            .token(POSTGRES_RESOURCE)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid expires_on \"soon\""),
            "{err:#}"
        );

        let endpoint = serve(Router::new().route("/msi/token", get(|| async { "<html>" }))).await;
        let err = Credential::app_service(&endpoint, "h")
            .token(POSTGRES_RESOURCE)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("decoding the managed identity token"),
            "{err:#}"
        );
    }

    #[test]
    fn managed_identity_needs_both_variables() {
        let app_service = |c: &Credential| match c {
            Credential::AppService {
                endpoint, header, ..
            } => Some((endpoint.clone(), header.clone())),
            Credential::AzureCli => None,
        };
        let some = |s: &str| Some(s.to_owned());

        assert_eq!(
            app_service(&Credential::from_vars(some("http://msi"), some("h"))),
            Some(("http://msi".into(), "h".into()))
        );
        assert_eq!(
            app_service(&Credential::from_vars(some("http://msi"), None)),
            None
        );
        assert_eq!(app_service(&Credential::from_vars(None, some("h"))), None);
        assert_eq!(app_service(&Credential::from_vars(None, None)), None);
    }

    /// A stand-in for `az` that runs `script` (a POSIX shell body).
    #[cfg(unix)]
    fn fake_az(dir: &std::path::Path, name: &str, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn azure_cli_token() {
        let dir = tempfile::tempdir().unwrap();

        // Asks for the resource's token, as JSON.
        let az = fake_az(
            dir.path(),
            "az-ok",
            r#"[ "$*" = "account get-access-token --output json --resource https://ossrdbms-aad.database.windows.net" ] || { echo "bad args: $*" >&2; exit 2; }
echo '{"accessToken":"from-cli","expires_on":1900000000,"tokenType":"Bearer"}'"#,
        );
        let token = cli_token(&az, POSTGRES_RESOURCE)
            .await
            .unwrap()
            .into_token()
            .unwrap();
        assert_eq!(token.secret, "from-cli");
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_900_000_000)
        );

        // Not logged in: the CLI's own explanation is the error.
        let az = fake_az(
            dir.path(),
            "az-logged-out",
            "echo \"ERROR: Please run 'az login' to setup account.\" >&2; exit 1",
        );
        let err = cli_token(&az, POSTGRES_RESOURCE).await.err().unwrap();
        assert_eq!(
            format!("{err:#}"),
            "az account get-access-token failed: ERROR: Please run 'az login' to setup account."
        );

        let az = fake_az(dir.path(), "az-garbage", "echo 'WARNING: not json'");
        let err = cli_token(&az, POSTGRES_RESOURCE).await.err().unwrap();
        assert!(
            format!("{err:#}").starts_with("decoding `az` token output"),
            "{err:#}"
        );

        let missing = dir.path().join("no-az");
        let err = cli_token(missing.to_str().unwrap(), POSTGRES_RESOURCE)
            .await
            .err()
            .unwrap();
        assert!(
            format!("{err:#}").starts_with("running `az` (is the Azure CLI installed?)"),
            "{err:#}"
        );
    }

    #[test]
    fn cli_output_is_parsed() {
        let raw: RawToken = serde_json::from_value(json!({
            "accessToken": "abc",
            "expiresOn": "2026-10-01 12:00:00.000000",
            "expires_on": 1_790_000_000,
            "tokenType": "Bearer",
        }))
        .unwrap();

        let token = raw.into_token().unwrap();
        assert_eq!(token.secret, "abc");
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_790_000_000)
        );
        assert!(!format!("{token:?}").contains("abc"));
    }
}
