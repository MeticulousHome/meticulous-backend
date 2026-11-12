//! Device authorization: which network clients may use the API and
//! Socket.IO.
//!
//! The machine itself (no `X-Real-IP`, or a loopback one) is always let in,
//! and so is a client on the machine's own hotspot, which is physically next
//! to it. Everyone else needs a device token while the
//! `require_authorization` setting is on. A token is an HS256 JWT signed with
//! a secret kept in the config directory: a factory reset (which wipes that
//! directory) or "Sign out all devices" (a new secret) logs every device
//! out, with nothing stored per device. A device gets its token by
//! exchanging a one-time pairing code, which the machine hands out on the
//! Dial, to an already authorized device, in the Improv Wi-Fi result, or
//! once someone allows an access request on the Dial. Codes and requests
//! live in memory only.
//!
//! Tokens, codes and the secret never reach the log.

use std::collections::VecDeque;
use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::config::{ConfigStore, keys};
use crate::limited_access::UnlockThrottle;
use crate::notifications::{Notification, NotificationManager, responses};
use crate::py::PyResult;
use crate::sio::Sio;
use crate::wifi::types::IpNet;
use crate::wifi::{WifiManager, WifiSystemConfig};

/// The signing secret's file in the config directory.
pub const KEY_FILE: &str = "device_auth.key";
const SECRET_BYTES: usize = 32;
/// Crockford base32: no I, L, O or U, which read like other characters.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_CHARS: usize = 8;
/// How long a pairing code can be exchanged.
pub const CODE_LIFETIME_S: u64 = 600;
/// Live codes at most; minting another drops the oldest.
pub const MAX_LIVE_CODES: usize = 16;
/// How long the Dial asks before an access request counts as refused.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Access requests accepted per [`REQUEST_WINDOW_S`]: each one makes the
/// machine ring and shows a question on the Dial.
pub const MAX_REQUESTS: usize = 5;
pub const REQUEST_WINDOW_S: f64 = 60.0;
/// How long a finished request's outcome stays readable.
pub const REQUEST_RECORD_S: f64 = 600.0;
/// The token's name when neither the device nor its code gave one.
pub const UNNAMED_DEVICE: &str = "Unnamed device";
/// How long the hotspot's subnet is reused before NetworkManager is asked
/// again: the guard may ask on every request.
const DIRECT_SUBNETS_TTL_S: f64 = 5.0;
const JWT_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// A device name a client sent (or a BLE name), as the Dial and the token
/// show it: no control characters and no `<`, `>` or `&`, trimmed, at most
/// 48 characters; `None` when nothing printable is left.
pub fn sanitize_name(name: &str) -> Option<String> {
    crate::ble::display_text(name, crate::ble::MAX_CLIENT_NAME_CHARS)
}

// ---- tokens ----

/// What a token says about the device holding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// The device id, a random UUID per token.
    pub sub: String,
    pub name: String,
    /// Unix seconds.
    pub iat: i64,
    pub exp: Option<i64>,
}

fn hmac(secret: &[u8]) -> Option<Hmac<Sha256>> {
    <Hmac<Sha256> as KeyInit>::new_from_slice(secret).ok()
}

/// The HS256 JWT for `claims`.
pub fn sign(secret: &[u8], claims: &Claims) -> Option<String> {
    let mut body = json!({"sub": claims.sub, "name": claims.name, "iat": claims.iat});
    if let Some(exp) = claims.exp {
        body["exp"] = json!(exp);
    }
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(JWT_HEADER),
        URL_SAFE_NO_PAD.encode(body.to_string())
    );
    let mut mac = hmac(secret)?;
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    Some(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// The claims of `token` when it is an HS256 JWT signed with `secret` that
/// has not expired at `now` (unix seconds). Any other algorithm, `none`
/// included, is refused before the signature is looked at, and the
/// signature is compared in constant time.
pub fn verify(secret: &[u8], token: &str, now: i64) -> Option<Claims> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let signing_input = &token[..token.rfind('.')?];
    let mut mac = hmac(secret)?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature).ok()?;

    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let sub = claims.get("sub")?.as_str().filter(|s| !s.is_empty())?;
    let name = claims.get("name")?.as_str()?;
    let iat = claims.get("iat")?.as_i64()?;
    let exp = match claims.get("exp") {
        None => None,
        Some(exp) => Some(exp.as_i64()?),
    };
    if exp.is_some_and(|exp| now >= exp) {
        return None;
    }
    Some(Claims {
        sub: sub.to_string(),
        name: name.to_string(),
        iat,
        exp,
    })
}

/// The token in an `Authorization: Bearer <token>` header.
pub fn bearer(headers: &http::HeaderMap) -> Option<&str> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// The secret in `path`, created on first use. A file of the wrong size is
/// replaced (every device then pairs again) rather than trusted.
fn read_or_create_secret(path: &Path) -> std::io::Result<[u8; SECRET_BYTES]> {
    match std::fs::read(path) {
        Ok(bytes) => match <[u8; SECRET_BYTES]>::try_from(bytes.as_slice()) {
            Ok(secret) => {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                Ok(secret)
            }
            Err(_) => {
                tracing::warn!("The device authorization key is damaged; replacing it");
                let secret = rand::random::<[u8; SECRET_BYTES]>();
                write_secret(path, &secret)?;
                Ok(secret)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_secret(path),
        Err(error) => Err(error),
    }
}

/// A new secret file, never overwriting one another process just made.
fn create_secret(path: &Path) -> std::io::Result<[u8; SECRET_BYTES]> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let secret = rand::random::<[u8; SECRET_BYTES]>();
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(&secret)?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(path)?;
            <[u8; SECRET_BYTES]>::try_from(bytes.as_slice()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "device authorization key of the wrong size",
                )
            })
        }
        Err(error) => Err(error),
    }
}

/// Replace the secret in one step, so a crash leaves the old key or the new
/// one, never half of either, and never readable by others.
fn write_secret(path: &Path, secret: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{KEY_FILE}."))
        .suffix(".tmp")
        .tempfile_in(dir)?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    temporary.write_all(secret)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    if let Err(error) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
        tracing::warn!("Could not fsync the directory of the device authorization key: {error}");
    }
    Ok(())
}

// ---- pairing codes ----

/// Who a pairing code was handed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSource {
    /// The Dial's "Pair a device" screen.
    New,
    /// An authorized device's "Allow another device".
    Grant,
    /// The Improv Wi-Fi result, after Connect on the Dial.
    Improv,
    /// An access request someone allowed on the Dial.
    Request,
}

impl CodeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeSource::New => "new",
            CodeSource::Grant => "grant",
            CodeSource::Improv => "improv",
            CodeSource::Request => "request",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairingCode {
    pub code: String,
    pub source: CodeSource,
    /// The name the token gets when the device sends none.
    pub name: Option<String>,
    minted: f64,
}

/// A typed code in its canonical form: upper case, without spaces or
/// dashes, with the look-alikes I, L and O read as 1, 1 and 0. `None` when
/// that is not a possible code.
pub fn normalize_code(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect();
    (code.len() == CODE_CHARS && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

fn random_code() -> String {
    (0..CODE_CHARS)
        .map(|_| char::from(CODE_ALPHABET[rand::random_range(0..CODE_ALPHABET.len())]))
        .collect()
}

// ---- access requests ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestStatus::Pending => "pending",
            RequestStatus::Approved => "approved",
            RequestStatus::Denied => "denied",
            RequestStatus::Expired => "expired",
        }
    }
}

struct AccessRequest {
    id: String,
    name: String,
    /// The Dial's question.
    notification: String,
    status: RequestStatus,
    created: f64,
    finished: Option<f64>,
    code: Option<String>,
}

#[derive(Default)]
struct Requests {
    list: Vec<AccessRequest>,
    /// When each recent request was accepted, oldest first.
    recent: VecDeque<f64>,
}

/// Why an access request was not put to the Dial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessRefusal {
    /// Another request still waits for its answer.
    Pending,
    /// Too many requests lately; seconds until the next one is taken.
    Throttled { retry_after: u64 },
}

/// A request's outcome as its requester reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessView {
    pub status: RequestStatus,
    /// With an approval: the code and the seconds it stays valid.
    pub code: Option<(String, u64)>,
}

// ---- who is asking ----

/// What device authorization knows about a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub local: bool,
    /// On the machine's own hotspot.
    pub direct: bool,
    pub required: bool,
    /// The device its token names, when it sent a valid one.
    pub device: Option<Claims>,
}

impl Admission {
    /// Whether the client may use the API and Socket.IO.
    pub fn admitted(&self) -> bool {
        self.local || self.direct || !self.required || self.device.is_some()
    }
}

/// A Socket.IO client's standing, kept on the socket so it can be
/// disconnected when that changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketAuth {
    pub local: bool,
    /// On the hotspot at the handshake. Only checked while authorization
    /// is required: with it off everyone connects, and a check would run
    /// NetworkManager for nothing.
    pub direct: bool,
    /// The client's address (`X-Real-IP`), so a sweep can still tell a
    /// hotspot client that connected while authorization was off.
    pub ip: Option<IpAddr>,
    /// The secret's generation the token was checked against, and its
    /// expiry.
    pub token: Option<(u64, Option<i64>)>,
}

/// Whether a connected socket may stay: local ones always; after "Sign out
/// all devices" no other; otherwise those the rules still admit, with
/// `hotspot` the subnets that count as direct now.
pub fn socket_may_stay(
    state: Option<&SocketAuth>,
    signed_out: bool,
    required: bool,
    generation: u64,
    now: i64,
    hotspot: &[IpNet],
) -> bool {
    let Some(state) = state else {
        return !signed_out && !required;
    };
    if state.local {
        return true;
    }
    if signed_out {
        return false;
    }
    let on_hotspot = || {
        state
            .ip
            .is_some_and(|ip| hotspot.iter().any(|subnet| in_subnet(subnet, ip)))
    };
    if !required || state.direct || on_hotspot() {
        return true;
    }
    state
        .token
        .is_some_and(|(issued, exp)| issued == generation && exp.is_none_or(|exp| now < exp))
}

/// The subnets of the connections the machine shares itself: the hotspot.
/// Empty when it runs none, so nobody is exempt by guesswork.
pub fn direct_subnets(config: &WifiSystemConfig) -> Vec<IpNet> {
    if !config.is_hotspot() {
        return Vec::new();
    }
    config
        .ips
        .iter()
        // A prefix this short is no hotspot NetworkManager made.
        .filter(|ip| ip.is_ipv4() && (16..32).contains(&ip.prefix))
        .cloned()
        .collect()
}

/// The client's address as nginx passes it (`X-Real-IP`).
fn real_ip(headers: &http::HeaderMap) -> Option<IpAddr> {
    headers
        .get("X-Real-IP")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<IpAddr>().ok())
}

/// Whether `ip` is in `subnet`; an IPv4-mapped IPv6 address counts as the
/// IPv4 one.
fn in_subnet(subnet: &IpNet, ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => return false,
        },
        v4 => v4,
    };
    subnet.is_ipv4()
        && IpNet {
            addr: ip,
            prefix: subnet.prefix,
        }
        .first()
            == subnet.first()
}

/// What device authorization reads about the network.
pub trait NetworkState: Send + Sync {
    /// `WifiManager.getCurrentConfig()`.
    fn current_config(&self) -> PyResult<WifiSystemConfig>;
}

impl NetworkState for WifiManager {
    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        self.get_current_config()
    }
}

/// With the hardware faked (tests, the package boot check) there is no
/// NetworkManager to ask: no hotspot, no address.
pub struct NoNetwork;

impl NetworkState for NoNetwork {
    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        Ok(WifiSystemConfig::disconnected(system_hostname()))
    }
}

fn system_hostname() -> String {
    nix::unistd::gethostname()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub struct DeviceAuth {
    me: Weak<DeviceAuth>,
    key_path: PathBuf,
    config: Arc<ConfigStore>,
    sio: Arc<Sio>,
    notifications: Arc<NotificationManager>,
    network: RwLock<Arc<dyn NetworkState>>,
    secret: Mutex<Option<[u8; SECRET_BYTES]>>,
    /// Counts the secret's rotations, so a socket can tell its token went
    /// stale.
    generation: AtomicU64,
    codes: Mutex<Vec<PairingCode>>,
    requests: Mutex<Requests>,
    /// Failed code exchanges, as `POST /machine/unlock` counts them.
    pub token_throttle: UnlockThrottle,
    /// The hotspot's subnets and when they were read.
    direct: Mutex<Option<(f64, Vec<IpNet>)>>,
    /// Held while NetworkManager is asked for them, so a burst of tokenless
    /// requests runs `nmcli` once, not once each.
    direct_refresh: tokio::sync::Mutex<()>,
    started: Instant,
    /// Added to the clock, so tests can let codes and tokens expire.
    skew_ms: AtomicU64,
}

impl DeviceAuth {
    pub fn new(
        key_path: PathBuf,
        config: Arc<ConfigStore>,
        sio: Arc<Sio>,
        notifications: Arc<NotificationManager>,
        network: Arc<dyn NetworkState>,
    ) -> Arc<DeviceAuth> {
        Arc::new_cyclic(|me| DeviceAuth {
            me: me.clone(),
            key_path,
            config,
            sio,
            notifications,
            network: RwLock::new(network),
            secret: Mutex::new(None),
            generation: AtomicU64::new(0),
            codes: Mutex::new(Vec::new()),
            requests: Mutex::new(Requests::default()),
            token_throttle: UnlockThrottle::default(),
            direct: Mutex::new(None),
            direct_refresh: tokio::sync::Mutex::new(()),
            started: Instant::now(),
            skew_ms: AtomicU64::new(0),
        })
    }

    #[cfg(test)]
    pub fn set_network(&self, network: Arc<dyn NetworkState>) {
        *self.network.write() = network;
        *self.direct.lock() = None;
    }

    /// Move the clock forward, for codes, requests and token expiry.
    #[cfg(test)]
    pub fn advance(&self, seconds: u64) {
        self.skew_ms.fetch_add(seconds * 1000, Ordering::Relaxed);
    }

    /// How many codes are live.
    #[cfg(test)]
    pub fn live_codes(&self) -> usize {
        let now = self.now();
        self.codes
            .lock()
            .iter()
            .filter(|c| now - c.minted < CODE_LIFETIME_S as f64)
            .count()
    }

    fn network(&self) -> Arc<dyn NetworkState> {
        self.network.read().clone()
    }

    /// Seconds on a monotonic clock.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64() + self.skew_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// Unix seconds, for the tokens.
    fn unix_now(&self) -> i64 {
        chrono::Utc::now().timestamp() + (self.skew_ms.load(Ordering::Relaxed) / 1000) as i64
    }

    /// The `require_authorization` setting.
    pub fn required(&self) -> bool {
        self.config
            .get_bool(keys::USER, keys::REQUIRE_AUTHORIZATION)
    }

    /// `system.auth_token_lifetime`, when tokens expire at all.
    fn lifetime(&self) -> Option<i64> {
        self.config
            .get_i64(keys::SYSTEM, keys::AUTH_TOKEN_LIFETIME)
            .filter(|seconds| *seconds > 0)
    }

    fn secret(&self) -> std::io::Result<[u8; SECRET_BYTES]> {
        let mut cached = self.secret.lock();
        if let Some(secret) = *cached {
            return Ok(secret);
        }
        let secret = read_or_create_secret(&self.key_path)?;
        *cached = Some(secret);
        Ok(secret)
    }

    // ---- tokens ----

    /// A token for a device called `name`, and its claims.
    pub fn issue(&self, name: &str) -> std::io::Result<(String, Claims)> {
        let secret = self.secret()?;
        let iat = self.unix_now();
        let claims = Claims {
            sub: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            iat,
            exp: self.lifetime().map(|lifetime| iat.saturating_add(lifetime)),
        };
        let token = sign(&secret, &claims)
            .ok_or_else(|| std::io::Error::other("the signing key was refused"))?;
        Ok((token, claims))
    }

    /// The claims of a token this machine issued, unless it expired or the
    /// devices were signed out since.
    pub fn verify(&self, token: &str) -> Option<Claims> {
        match self.secret() {
            Ok(secret) => verify(&secret, token, self.unix_now()),
            Err(error) => {
                tracing::error!("Could not read the device authorization key: {error}");
                None
            }
        }
    }

    /// The device behind the request's bearer token, when it is valid.
    pub fn device(&self, headers: &http::HeaderMap) -> Option<Claims> {
        self.verify(bearer(headers)?)
    }

    /// Whether the client is let in, and why. The hotspot is only looked at
    /// when nothing else admits the client.
    pub async fn admission(&self, headers: &http::HeaderMap) -> Admission {
        let local = crate::api::is_local(headers);
        let required = self.required();
        let device = self.device(headers);
        let direct = !local && required && device.is_none() && self.is_direct(headers).await;
        Admission {
            local,
            direct,
            required,
            device,
        }
    }

    /// Whether the client (`X-Real-IP`) is on the machine's hotspot.
    async fn is_direct(&self, headers: &http::HeaderMap) -> bool {
        let Some(ip) = real_ip(headers) else {
            return false;
        };
        self.direct_subnets()
            .await
            .iter()
            .any(|subnet| in_subnet(subnet, ip))
    }

    /// The hotspot's subnets, read at most once per
    /// [`DIRECT_SUBNETS_TTL_S`] by one caller while the others wait for it.
    /// A failed read keeps the last subnets for another TTL, so a failing
    /// `nmcli` is not run again on every request.
    async fn direct_subnets(&self) -> Vec<IpNet> {
        if let Some(subnets) = self.fresh_direct_subnets() {
            return subnets;
        }
        let _refreshing = self.direct_refresh.lock().await;
        // Whoever held the lock before may have just read them.
        if let Some(subnets) = self.fresh_direct_subnets() {
            return subnets;
        }
        let network = self.network();
        let read = tokio::task::spawn_blocking(move || {
            network
                .current_config()
                .map(|config| direct_subnets(&config))
        })
        .await;
        let subnets = match read {
            Ok(Ok(subnets)) => subnets,
            Ok(Err(error)) => {
                tracing::warn!("Could not read the network state: {}", error.kind);
                self.last_direct_subnets()
            }
            Err(error) => {
                tracing::warn!("Could not read the network state: {error}");
                self.last_direct_subnets()
            }
        };
        *self.direct.lock() = Some((self.now(), subnets.clone()));
        subnets
    }

    fn fresh_direct_subnets(&self) -> Option<Vec<IpNet>> {
        let now = self.now();
        match &*self.direct.lock() {
            Some((read_at, subnets)) if now - read_at < DIRECT_SUBNETS_TTL_S => {
                Some(subnets.clone())
            }
            _ => None,
        }
    }

    fn last_direct_subnets(&self) -> Vec<IpNet> {
        self.direct
            .lock()
            .as_ref()
            .map(|(_, subnets)| subnets.clone())
            .unwrap_or_default()
    }

    // ---- Socket.IO ----

    /// The standing of a Socket.IO client from its handshake: the token in
    /// the CONNECT payload (`{token}`) or an `Authorization` header. `None`
    /// when it must be refused.
    pub async fn admit_socket(
        &self,
        headers: &http::HeaderMap,
        auth: Option<&Value>,
    ) -> Option<SocketAuth> {
        let token = auth
            .and_then(|auth| auth.get("token"))
            .and_then(Value::as_str)
            .or_else(|| bearer(headers));
        let generation = self.generation.load(Ordering::Relaxed);
        let device = token.and_then(|token| self.verify(token));
        let local = crate::api::is_local(headers);
        let required = self.required();
        let direct = !local && required && device.is_none() && self.is_direct(headers).await;
        let state = SocketAuth {
            local,
            direct,
            ip: real_ip(headers),
            token: device.map(|claims| (generation, claims.exp)),
        };
        let admitted = local || direct || state.token.is_some() || !required;
        if admitted {
            self.sweep_at_expiry(&state);
        }
        admitted.then_some(state)
    }

    /// How long after now a socket admitted as `state` must be looked at
    /// again: a second past its token's expiry. `None` when nothing about
    /// it expires (local, hotspot, or a token without `exp`).
    pub fn expiry_sweep_delay(&self, state: &SocketAuth) -> Option<Duration> {
        if state.local || state.direct {
            return None;
        }
        let (_, Some(exp)) = state.token? else {
            return None;
        };
        let left = exp.saturating_sub(self.unix_now()).max(0) as u64;
        Some(Duration::from_secs(left + 1))
    }

    /// The handshake checks a token once; without this an open socket keeps
    /// every broadcast and can still send actions after the token expired,
    /// which HTTP refuses. The sweep waits for `exp` on the wall clock the
    /// tokens use, sleeping again if that clock was set back meanwhile.
    fn sweep_at_expiry(&self, state: &SocketAuth) {
        let (Some(mut delay), Some((_, Some(exp)))) = (self.expiry_sweep_delay(state), state.token)
        else {
            return;
        };
        let auth = self.me.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(delay).await;
                let Some(auth) = auth.upgrade() else {
                    return;
                };
                let now = auth.unix_now();
                if now >= exp {
                    auth.sweep_sockets(false).await;
                    return;
                }
                delay = Duration::from_secs((exp - now) as u64 + 1);
            }
        });
    }

    /// Disconnect the sockets that may no longer stay (see
    /// [`socket_may_stay`]): every non-local one after a sign-out, else
    /// those without a valid token once authorization is required. Safe to
    /// run any time: it drops only what the rules refuse now.
    pub async fn sweep_sockets(&self, signed_out: bool) {
        let required = self.required();
        // Sockets let in while authorization was off were never checked
        // against the hotspot.
        let hotspot = if required && !signed_out {
            self.direct_subnets().await
        } else {
            Vec::new()
        };
        self.disconnect_refused(signed_out, required, &hotspot);
    }

    fn disconnect_refused(&self, signed_out: bool, required: bool, hotspot: &[IpNet]) {
        let generation = self.generation.load(Ordering::Relaxed);
        let now = self.unix_now();
        let dropped = self.sio.disconnect_where(|socket| {
            let state = socket.extensions.get::<SocketAuth>();
            !socket_may_stay(
                state.as_ref(),
                signed_out,
                required,
                generation,
                now,
                hotspot,
            )
        });
        if dropped > 0 {
            tracing::info!("Disconnected {dropped} Socket.IO client(s) without authorization");
        }
    }

    // ---- pairing codes ----

    /// A new one-time code; the oldest live one goes when there are too
    /// many.
    pub fn mint_code(&self, source: CodeSource, name: Option<String>) -> String {
        let now = self.now();
        let mut codes = self.codes.lock();
        codes.retain(|c| now - c.minted < CODE_LIFETIME_S as f64);
        while codes.len() >= MAX_LIVE_CODES {
            codes.remove(0);
        }
        let code = loop {
            let code = random_code();
            if !codes.iter().any(|c| c.code == code) {
                break code;
            }
        };
        codes.push(PairingCode {
            code: code.clone(),
            source,
            name,
            minted: now,
        });
        tracing::info!("Pairing code issued (source={})", source.as_str());
        code
    }

    /// The live code `input` names, used up.
    pub fn take_code(&self, input: &str) -> Option<PairingCode> {
        let code = normalize_code(input)?;
        let now = self.now();
        let mut codes = self.codes.lock();
        codes.retain(|c| now - c.minted < CODE_LIFETIME_S as f64);
        let index = codes.iter().position(|c| c.code == code)?;
        Some(codes.remove(index))
    }

    /// Whole seconds `code` stays valid; 0 once it is used or expired.
    fn code_expires_in(&self, code: &str) -> u64 {
        let now = self.now();
        self.codes
            .lock()
            .iter()
            .find(|c| c.code == code)
            .map(|c| (CODE_LIFETIME_S as f64 - (now - c.minted)).ceil().max(0.0) as u64)
            .unwrap_or(0)
    }

    /// `http://<host>/#pair=<code>`: the first IPv4 address of the current
    /// connection, else `<hostname>.local`. Asks NetworkManager: blocking.
    pub fn pair_url(&self, code: &str) -> String {
        let host = match self.network().current_config() {
            Ok(config) => {
                let ipv4 = config.ips.iter().find_map(|ip| match ip.addr {
                    IpAddr::V4(v4) => Some(v4.to_string()),
                    IpAddr::V6(_) => None,
                });
                ipv4.unwrap_or_else(|| {
                    let name = if config.hostname.is_empty() {
                        system_hostname()
                    } else {
                        config.hostname
                    };
                    format!("{name}.local")
                })
            }
            Err(_) => format!("{}.local", system_hostname()),
        };
        format!("http://{host}/#pair={code}")
    }

    // ---- access requests ----

    /// Ask on the Dial whether `name` may use the machine. The answer comes
    /// back through the notification's callback.
    pub fn request_access(&self, name: &str) -> Result<String, AccessRefusal> {
        self.expire_overdue();
        let now = self.now();
        let id = uuid::Uuid::new_v4().to_string();
        let notification = {
            let mut requests = self.requests.lock();
            requests
                .list
                .retain(|r| r.finished.is_none_or(|at| now - at < REQUEST_RECORD_S));
            if requests
                .list
                .iter()
                .any(|r| r.status == RequestStatus::Pending)
            {
                return Err(AccessRefusal::Pending);
            }
            while requests
                .recent
                .front()
                .is_some_and(|at| now - at >= REQUEST_WINDOW_S)
            {
                requests.recent.pop_front();
            }
            if requests.recent.len() >= MAX_REQUESTS {
                let oldest = requests.recent.front().copied().unwrap_or(now);
                let retry_after = (oldest + REQUEST_WINDOW_S - now).ceil().max(1.0) as u64;
                return Err(AccessRefusal::Throttled { retry_after });
            }
            requests.recent.push_back(now);
            let me = self.me.clone();
            let answered_id = id.clone();
            let notification = Notification::with_responses(
                format!("\u{201c}{name}\u{201d} is asking to use this machine"),
                &[responses::DENY, responses::ALLOW],
            )
            // PARITY: acknowledging runs the callback twice; the second call
            // finds the request answered already.
            .callback(Arc::new(move || {
                if let Some(auth) = me.upgrade() {
                    auth.answered(&answered_id);
                }
            }))
            .private()
            .answer_locally();
            requests.list.push(AccessRequest {
                id: id.clone(),
                name: name.to_string(),
                notification: notification.id.clone(),
                status: RequestStatus::Pending,
                created: now,
                finished: None,
                code: None,
            });
            notification
        };
        self.notifications.add(notification);
        // The id lets its holder read the code: it stays out of the log.
        tracing::info!("An access request is waiting for the Dial");
        if let (Ok(runtime), Some(auth)) =
            (tokio::runtime::Handle::try_current(), self.me.upgrade())
        {
            let id = id.clone();
            runtime.spawn(async move {
                tokio::time::sleep(REQUEST_TIMEOUT).await;
                auth.expire(|r| r.id == id);
            });
        }
        Ok(id)
    }

    /// The Dial answered request `id`.
    fn answered(&self, id: &str) {
        let pending = {
            let requests = self.requests.lock();
            requests
                .list
                .iter()
                .find(|r| r.id == id && r.status == RequestStatus::Pending)
                .map(|r| (r.notification.clone(), r.name.clone()))
        };
        let Some((notification, name)) = pending else {
            return;
        };
        let response = self
            .notifications
            .all()
            .into_iter()
            .find(|n| n.id == notification)
            .and_then(|n| n.response);
        let approved = response.as_deref() == Some(responses::ALLOW);
        let code = approved.then(|| self.mint_code(CodeSource::Request, Some(name)));
        let now = self.now();
        let mut requests = self.requests.lock();
        if let Some(request) = requests
            .list
            .iter_mut()
            .find(|r| r.id == id && r.status == RequestStatus::Pending)
        {
            request.status = if approved {
                RequestStatus::Approved
            } else {
                RequestStatus::Denied
            };
            request.finished = Some(now);
            request.code = code;
            tracing::info!(
                "Access request {} on the Dial",
                if approved { "allowed" } else { "denied" }
            );
        }
    }

    /// Requests nobody answered in time count as refused.
    fn expire_overdue(&self) {
        let now = self.now();
        self.expire(|r| now - r.created >= REQUEST_TIMEOUT.as_secs_f64());
    }

    /// Expire the pending requests `which` selects, turning the Dial's
    /// question into a notice under the same id: the Dial only drops a
    /// notification once it is answered or replaced.
    fn expire(&self, which: impl Fn(&AccessRequest) -> bool) {
        let now = self.now();
        let expired: Vec<(String, String)> = {
            let mut requests = self.requests.lock();
            requests
                .list
                .iter_mut()
                .filter(|r| r.status == RequestStatus::Pending && which(r))
                .map(|r| {
                    r.status = RequestStatus::Expired;
                    r.finished = Some(now);
                    (r.notification.clone(), r.name.clone())
                })
                .collect()
        };
        for (notification, name) in expired {
            tracing::info!("Access request not answered in time; refusing it");
            let mut notice = Notification::new(format!(
                "\u{201c}{name}\u{201d} was not allowed to use this machine: nobody answered in time"
            ))
            .private();
            notice.id = notification;
            self.notifications.add(notice);
        }
    }

    /// Request `id`'s outcome; `None` once it is forgotten or never was.
    pub fn access_request(&self, id: &str) -> Option<AccessView> {
        self.expire_overdue();
        let now = self.now();
        let (status, code) = {
            let mut requests = self.requests.lock();
            requests
                .list
                .retain(|r| r.finished.is_none_or(|at| now - at < REQUEST_RECORD_S));
            let request = requests.list.iter().find(|r| r.id == id)?;
            (request.status, request.code.clone())
        };
        let code = match (status, code) {
            (RequestStatus::Approved, Some(code)) => {
                let expires_in = self.code_expires_in(&code);
                Some((code, expires_in))
            }
            _ => None,
        };
        Some(AccessView { status, code })
    }

    // ---- sign out ----

    /// "Sign out all devices": a new secret (every token stops working),
    /// no live codes, no pending requests, and every non-local socket
    /// disconnected.
    pub fn sign_out_all(&self) -> std::io::Result<()> {
        let secret = rand::random::<[u8; SECRET_BYTES]>();
        {
            let mut cached = self.secret.lock();
            write_secret(&self.key_path, &secret)?;
            *cached = Some(secret);
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        self.codes.lock().clear();
        let now = self.now();
        let dropped: Vec<String> = {
            let mut requests = self.requests.lock();
            requests
                .list
                .iter_mut()
                .filter(|r| r.status == RequestStatus::Pending)
                .map(|r| {
                    r.status = RequestStatus::Denied;
                    r.finished = Some(now);
                    r.notification.clone()
                })
                .collect()
        };
        for notification in dropped {
            // An empty message removes the question from the Dial.
            let mut gone = Notification::new("");
            gone.id = notification.clone();
            self.notifications.add(gone);
            self.notifications.acknowledge(&notification, None);
        }
        tracing::info!("Signed out all devices");
        // Every non-local socket goes: no need to know the hotspot.
        self.disconnect_refused(true, self.required(), &[]);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
