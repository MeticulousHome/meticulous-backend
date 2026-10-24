//! Outbound HTTPS for the network modules: the Wi-Fi internet probe
//! (`urllib.request.urlopen`) and the IP timezone lookup (`aiohttp`). Both
//! run on blocking threads, like the Python's, so the client is blocking at
//! the boundary and async inside.

use std::time::Duration;

/// Why a request produced no response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("timed out")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

/// A finished request: the status after redirects and the body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait HttpClient: Send + Sync {
    /// `GET url`, following redirects, giving up after `timeout`.
    fn get(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<HttpResponse, HttpError>;

    /// `GET url` for its status only: the body is not read.
    fn status(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<u16, HttpError>;
}

/// reqwest on the backend's tokio runtime.
pub struct ReqwestClient {
    client: Option<reqwest::Client>,
    runtime: Option<tokio::runtime::Handle>,
}

impl ReqwestClient {
    /// Built inside the runtime (from `App::new`), so blocking threads can
    /// drive requests on it.
    pub fn new() -> ReqwestClient {
        let client = match reqwest::Client::builder().build() {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!("HTTPS client unavailable: {error}");
                None
            }
        };
        ReqwestClient {
            client,
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        ReqwestClient::new()
    }
}

impl HttpClient for ReqwestClient {
    fn get(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<HttpResponse, HttpError> {
        let (runtime, request) = self.request(url, timeout, user_agent)?;
        runtime.block_on(async move {
            let response = request.send().await.map_err(classify)?;
            let status = response.status().as_u16();
            let body = response.text().await.map_err(classify)?;
            Ok(HttpResponse { status, body })
        })
    }

    fn status(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<u16, HttpError> {
        let (runtime, request) = self.request(url, timeout, user_agent)?;
        runtime.block_on(async move {
            let response = request.send().await.map_err(classify)?;
            Ok(response.status().as_u16())
        })
    }
}

impl ReqwestClient {
    fn request(
        &self,
        url: &str,
        timeout: Duration,
        user_agent: Option<&str>,
    ) -> Result<(&tokio::runtime::Handle, reqwest::RequestBuilder), HttpError> {
        let (Some(runtime), Some(client)) = (&self.runtime, &self.client) else {
            return Err(HttpError::Failed("no HTTPS client".into()));
        };
        let mut request = client.get(url).timeout(timeout);
        if let Some(agent) = user_agent {
            request = request.header(reqwest::header::USER_AGENT, agent);
        }
        Ok((runtime, request))
    }
}

fn classify(error: reqwest::Error) -> HttpError {
    if error.is_timeout() {
        HttpError::Timeout
    } else {
        HttpError::Failed(error.to_string())
    }
}
