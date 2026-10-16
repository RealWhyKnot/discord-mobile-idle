use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{sleep, sleep_until, timeout, Instant};
use wreq::header::OrigHeaderMap;
use wreq::ws::message::Message;
use wreq::ws::WebSocket;
use wreq::Method;
use wreq_util::{Emulation, Platform, Profile};

use crate::controller::{INVISIBLE, OFFLINE};
use crate::persist::write_atomic;
use crate::proto;
use crate::runner::{self, beat_reader, Error, Note, Watchdog, UNKNOWN};
use crate::state::{SessionPayload, State};
use crate::{info, warning};

pub const CHROME: u32 = 149;
const API: &str = "https://discord.com/api/v9";
const GATEWAY: &str = "wss://gateway.discord.gg/";
const GATEWAY_QUERY: &str = "?v=9&encoding=json&compress=zlib-stream";
const CAPABILITIES: u64 = 22525;
const FALLBACK_BUILD: u64 = 9999;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SCRAPE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const TRIES: u64 = 5;
const ZLIB_SUFFIX: [u8; 4] = [0, 0, 0xff, 0xff];
const FATAL_CLOSES: [u16; 5] = [4004, 4010, 4011, 4012, 4013];
const INTENTS_CLOSE: u16 = 4014;
const BUFFER_KEEP: usize = 64 * 1024;
const INFLATE_CHUNK: usize = 16 * 1024;

const REST_ORDER: [&str; 18] = [
    "Sec-CH-UA",
    "Sec-CH-UA-Mobile",
    "Sec-CH-UA-Platform",
    "Accept",
    "Accept-Language",
    "Accept-Encoding",
    "Origin",
    "Priority",
    "Referer",
    "Sec-Fetch-Dest",
    "Sec-Fetch-Mode",
    "Sec-Fetch-Site",
    "User-Agent",
    "X-Discord-Locale",
    "X-Super-Properties",
    "Authorization",
    "Content-Type",
    "X-Failed-Requests",
];

const WS_ORDER: [&str; 12] = [
    "Host",
    "Connection",
    "Pragma",
    "Cache-Control",
    "User-Agent",
    "Upgrade",
    "Origin",
    "Sec-WebSocket-Version",
    "Accept-Encoding",
    "Accept-Language",
    "Sec-WebSocket-Key",
    "Sec-WebSocket-Extensions",
];

fn order(names: &[&'static str]) -> OrigHeaderMap {
    let mut map = OrigHeaderMap::new();
    for name in names {
        map.insert(*name);
    }
    map
}

fn emulation(http2: bool, headers: bool) -> Emulation {
    Emulation::builder()
        .profile(Profile::Chrome149)
        .platform(Platform::Windows)
        .http2(http2)
        .headers(headers)
        .build()
}

pub fn random_u64() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(std::process::id() as u64);
    hasher.finish()
}

fn uniform(high: f64) -> f64 {
    (random_u64() >> 11) as f64 / (1u64 << 53) as f64 * high
}

pub fn uuid4() -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&random_u64().to_le_bytes());
    bytes[8..].copy_from_slice(&random_u64().to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub fn user_agent(version: u32) -> String {
    format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        version
    )
}

pub fn sec_ch_ua(version: u32) -> String {
    const CHARS: [char; 11] = [' ', '(', ':', '-', '.', '/', ')', ';', '=', '?', '_'];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let v = version as usize;
    let greased = format!("Not{}A{}Brand", CHARS[v % 11], CHARS[(v + 1) % 11]);
    let major = version.to_string();
    let brands = [
        (greased.as_str(), VERSIONS[v % 3]),
        ("Chromium", major.as_str()),
        ("Google Chrome", major.as_str()),
    ];
    let order = ORDERS[v % 6];
    let mut shuffled = [("", ""); 3];
    for (i, brand) in brands.iter().enumerate() {
        shuffled[order[i]] = *brand;
    }
    shuffled
        .iter()
        .map(|(name, ver)| format!("\"{}\";v=\"{}\"", name, ver))
        .collect::<Vec<_>>()
        .join(", ")
}

fn object(pairs: &[(&str, Value)]) -> String {
    let fields: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}:{}", Value::from(*key), value))
        .collect();
    format!("{{{}}}", fields.join(","))
}

pub fn super_properties(version: u32, build: u64, launch: &str, heartbeat_session: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("os", json!("Windows")),
        ("browser", json!("Chrome")),
        ("device", json!("")),
        ("system_locale", json!("en-US")),
        ("browser_user_agent", json!(user_agent(version))),
        ("browser_version", json!(format!("{}.0.0.0", version))),
        ("os_version", json!("10")),
        ("referrer", json!("")),
        ("referring_domain", json!("")),
        ("referrer_current", json!("")),
        ("referring_domain_current", json!("")),
        ("release_channel", json!("stable")),
        ("client_build_number", json!(build)),
        ("client_event_source", Value::Null),
        ("has_client_mods", json!(false)),
        ("client_launch_id", json!(launch)),
        ("client_app_state", json!("unfocused")),
        ("client_heartbeat_session_id", json!(heartbeat_session)),
    ]
}

pub fn gateway_properties(mut base: Vec<(&'static str, Value)>) -> String {
    base.push(("is_fast_connect", json!(false)));
    base.push(("latest_headless_tasks", json!([])));
    base.push(("latest_headless_task_run_seconds_before", Value::Null));
    base.push(("gateway_connect_reasons", json!("AppSkeleton")));
    object(&base)
}

pub fn find_sentry(page: &str) -> Option<&str> {
    let mut rest = page;
    while let Some(at) = rest.find("assets/sentry.") {
        let name_start = at + "assets/".len();
        let tail = &rest[name_start + "sentry.".len()..];
        let word = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(tail.len());
        if word > 0 && tail[word..].starts_with(".js") {
            return Some(&rest[name_start..name_start + "sentry.".len() + word]);
        }
        rest = &rest[name_start..];
    }
    None
}

pub fn find_build_number(script: &str) -> Option<u64> {
    let mut rest = script;
    while let Some(at) = rest.find("buildNumber") {
        let tail = &rest[at + "buildNumber".len()..];
        let start = tail.find(|c: char| c.is_ascii_digit())?;
        let digits = &tail[start..];
        let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
        if digits[end..].starts_with('"') {
            if let Ok(number) = digits[..end].parse() {
                return Some(number);
            }
        }
        rest = tail;
    }
    None
}

fn error_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value.get("message").and_then(Value::as_str) {
            return message.to_string();
        }
    }
    body.chars().take(200).collect()
}

fn retry_after(body: &str) -> f64 {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("retry_after").and_then(Value::as_f64))
        .unwrap_or(1.0)
        .max(0.0)
}

pub struct Http {
    pub client: wreq::Client,
    token: String,
    user_agent: String,
    sec_ch_ua: String,
    launch_id: String,
    heartbeat_session_id: String,
    super_properties: RefCell<String>,
    properties: RefCell<String>,
}

impl Http {
    pub fn new(token: String) -> Result<Self, Error> {
        let client = wreq::Client::builder()
            .emulation(emulation(true, true))
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|e| Error::new(format!("could not build the HTTP client: {}", e)))?;
        let http = Http {
            client,
            token,
            user_agent: user_agent(CHROME),
            sec_ch_ua: sec_ch_ua(CHROME),
            launch_id: uuid4(),
            heartbeat_session_id: uuid4(),
            super_properties: RefCell::new(String::new()),
            properties: RefCell::new(String::new()),
        };
        http.set_build(FALLBACK_BUILD);
        Ok(http)
    }

    fn set_build(&self, build: u64) {
        let base = super_properties(CHROME, build, &self.launch_id, &self.heartbeat_session_id);
        *self.super_properties.borrow_mut() = proto::b64encode(object(&base).as_bytes());
        *self.properties.borrow_mut() = gateway_properties(base);
    }

    async fn scrape(&self, url: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .timeout(SCRAPE_TIMEOUT)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        response.text().await.map_err(|e| e.to_string())
    }

    async fn build_number(&self) -> Result<u64, String> {
        let page = self.scrape("https://discord.com/login").await?;
        let sentry = find_sentry(&page)
            .ok_or("no sentry script on the login page")?
            .to_string();
        drop(page);
        let script = self
            .scrape(&format!("https://static.discord.com/assets/{}.js", sentry))
            .await?;
        find_build_number(&script).ok_or_else(|| "no build number in the sentry script".to_string())
    }

    pub async fn startup(&self) {
        match self.build_number().await {
            Ok(build) => {
                info!("discord client build {}", build);
                self.set_build(build);
            }
            Err(e) => warning!(
                "could not fetch the client build number, using {}: {}",
                FALLBACK_BUILD,
                e
            ),
        }
    }

    pub fn properties(&self) -> String {
        self.properties.borrow().clone()
    }

    pub async fn request(&self, method: Method, path: &str, body: Option<String>) -> Result<String, Error> {
        let url = format!("{}{}", API, path);
        let mut failed = 0u64;
        let mut last = Error::new("no response");
        for tries in 0..TRIES {
            let mut builder = self
                .client
                .request(method.clone(), &url)
                .default_headers(false)
                .orig_headers(order(&REST_ORDER))
                .timeout(REQUEST_TIMEOUT)
                .header("Sec-CH-UA", &self.sec_ch_ua)
                .header("Sec-CH-UA-Mobile", "?0")
                .header("Sec-CH-UA-Platform", "\"Windows\"")
                .header("Accept", "*/*")
                .header("Accept-Language", "en-US,en;q=0.9")
                .header("Accept-Encoding", "gzip, deflate, br, zstd")
                .header("Origin", "https://discord.com")
                .header("Priority", "u=0, i")
                .header("Referer", "https://discord.com/channels/@me")
                .header("Sec-Fetch-Dest", "empty")
                .header("Sec-Fetch-Mode", "cors")
                .header("Sec-Fetch-Site", "same-origin")
                .header("User-Agent", &self.user_agent)
                .header("X-Discord-Locale", "en-US")
                .header("X-Super-Properties", self.super_properties.borrow().as_str())
                .header("Authorization", &self.token);
            if let Some(body) = &body {
                builder = builder.header("Content-Type", "application/json").body(body.clone());
            }
            if failed > 0 {
                builder = builder.header("X-Failed-Requests", failed.to_string());
            }
            let response = match builder.send().await {
                Ok(response) => response,
                Err(e) if tries + 1 < TRIES && (e.is_connect() || e.is_request()) => {
                    last = Error::new(e.to_string());
                    failed += 1;
                    sleep(Duration::from_secs(1 + tries * 2)).await;
                    continue;
                }
                Err(e) => return Err(Error::new(e.to_string())),
            };
            let status = response.status().as_u16();
            let text = response.text().await.map_err(|e| Error::new(e.to_string()))?;
            if (200..300).contains(&status) {
                return Ok(text);
            }
            if status == 429 {
                let wait = retry_after(&text);
                warning!("rate limited on {} {}, retrying in {:.2}s", method, path, wait);
                last = Error::http(status, error_message(&text));
                sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            if matches!(status, 500 | 502 | 504 | 524) {
                last = Error::http(status, error_message(&text));
                failed += 1;
                sleep(Duration::from_secs(1 + tries * 2)).await;
                continue;
            }
            return Err(Error::http(status, error_message(&text)));
        }
        Err(last)
    }

    pub async fn login(&self) -> Result<User, Error> {
        let body = self
            .request(Method::GET, "/users/@me?with_analytics_token=true", None)
            .await?;
        serde_json::from_str(&body).map_err(|e| Error::new(format!("could not read the account: {}", e)))
    }

    pub async fn check_token(&self) -> Result<(), Error> {
        self.request(Method::GET, "/users/@me?with_analytics_token=false", None)
            .await
            .map(|_| ())
    }

    pub async fn patch_settings(&self, settings: &[u8]) -> Result<(), Error> {
        let body = object(&[("settings", json!(proto::b64encode(settings)))]);
        self.request(Method::PATCH, "/users/@me/settings-proto/1", Some(body))
            .await
            .map(|_| ())
    }
}

#[derive(Deserialize, Default)]
pub struct User {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub discriminator: String,
}

impl User {
    pub fn display(&self) -> String {
        if self.discriminator.is_empty() || self.discriminator == "0" {
            self.username.clone()
        } else {
            format!("{}#{}", self.username, self.discriminator)
        }
    }
}

pub struct Shared {
    pub state: State,
    pub user: String,
    pub connected: bool,
}

pub type Link = Rc<RefCell<Shared>>;

pub fn link() -> Link {
    Rc::new(RefCell::new(Shared {
        state: State::new(),
        user: String::new(),
        connected: false,
    }))
}

pub enum Command {
    Send(String),
    Close,
}

fn presence_activities(activities: &[Value]) -> Value {
    Value::Array(
        activities
            .iter()
            .map(|activity| {
                if activity.get("type").and_then(Value::as_u64) != Some(4) {
                    return activity.clone();
                }
                let mut custom = serde_json::Map::new();
                custom.insert("type".into(), json!(4));
                custom.insert("state".into(), activity.get("state").cloned().unwrap_or(Value::Null));
                custom.insert("name".into(), json!("Custom Status"));
                if let Some(emoji) = activity.get("emoji").filter(|e| !e.is_null()) {
                    custom.insert("emoji".into(), emoji.clone());
                }
                Value::Object(custom)
            })
            .collect(),
    )
}

pub fn presence_payload(status: &str, activities: &[Value]) -> String {
    let d = object(&[
        ("activities", presence_activities(activities)),
        ("afk", json!(true)),
        ("since", json!(0)),
        ("status", json!(status)),
    ]);
    format!("{{\"op\":3,\"d\":{}}}", d)
}

pub struct DiscordClient {
    pub link: Link,
    pub http: Rc<Http>,
    pub commands: UnboundedSender<Command>,
}

impl DiscordClient {
    async fn presence(&self, status: &str, edit_settings: bool) -> Result<(), Error> {
        let sent = if status == OFFLINE { INVISIBLE } else { status };
        let (payload, patch) = {
            let mut shared = self.link.borrow_mut();
            if !shared.connected {
                return Err(Error::new("not connected to the gateway"));
            }
            if shared.state.ws_status == UNKNOWN {
                shared.state.ws_activities = shared.state.client_activities();
            }
            let payload = presence_payload(sent, &shared.state.ws_activities);
            shared.state.ws_status = sent.to_string();
            let patch = match &shared.state.settings {
                Some(settings) if edit_settings && proto::status(settings) != status => {
                    Some(proto::status_patch(settings, status))
                }
                _ => None,
            };
            (payload, patch)
        };
        self.commands
            .send(Command::Send(payload))
            .map_err(|_| Error::new("the gateway has stopped"))?;
        if let Some(patch) = patch {
            self.http.patch_settings(&patch).await?;
        }
        Ok(())
    }
}

impl runner::Client for DiscordClient {
    fn is_on_mobile(&self) -> bool {
        self.link.borrow().state.is_on_mobile()
    }

    fn other_sessions(&self) -> usize {
        self.link.borrow().state.other_sessions()
    }

    fn status(&self) -> String {
        self.link.borrow().state.status()
    }

    fn saved_status(&self) -> String {
        self.link.borrow().state.saved_status()
    }

    fn own_status(&self) -> Option<String> {
        self.link.borrow().state.own_status()
    }

    fn games(&self) -> Vec<String> {
        self.link.borrow().state.games()
    }

    async fn check_token(&self) -> Result<(), Error> {
        self.http.check_token().await
    }

    async fn change_presence(&self, status: &str) -> Result<(), Error> {
        self.presence(status, true).await
    }

    async fn hide(&self) -> Result<(), Error> {
        self.presence(INVISIBLE, false).await
    }

    async fn unhide(&self, status: &str) -> Result<(), Error> {
        self.presence(status, false).await
    }
}

struct Backoff {
    exp: u32,
    last: Instant,
}

impl Backoff {
    fn new() -> Self {
        Backoff {
            exp: 0,
            last: Instant::now(),
        }
    }

    fn delay(&mut self) -> f64 {
        let now = Instant::now();
        if now - self.last > Duration::from_secs(2048) {
            self.exp = 0;
        }
        self.last = now;
        self.exp = (self.exp + 1).min(10);
        uniform(f64::from(1u32 << self.exp))
    }
}

#[derive(Debug, PartialEq)]
enum End {
    Closed,
    Reconnect { resume: bool },
    Fatal(u16),
    Failed(String),
}

enum Body {
    Missing,
    Raw(Box<RawValue>),
    Ready(Box<Ready>),
    Skipped,
}

impl Body {
    fn text(&self) -> &str {
        match self {
            Body::Raw(raw) => raw.get(),
            _ => "null",
        }
    }

    fn into_ready(self) -> serde_json::Result<Ready> {
        match self {
            Body::Ready(ready) => Ok(*ready),
            other => serde_json::from_str(other.text()),
        }
    }
}

struct Frame {
    op: u8,
    s: Option<u64>,
    body: Body,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum Key {
    Op,
    S,
    T,
    D,
    #[serde(other)]
    Other,
}

const KEPT_EVENTS: [&str; 3] = ["SESSIONS_REPLACE", "USER_SETTINGS_PROTO_UPDATE", "USER_UPDATE"];

#[derive(Clone, Copy)]
enum BodySeed {
    Raw,
    Ready,
    Skip,
}

impl BodySeed {
    fn pick(op: Option<u8>, event: Option<&str>) -> Self {
        match (op, event) {
            (Some(0), Some("READY")) => BodySeed::Ready,
            (Some(0), Some(event)) if !KEPT_EVENTS.contains(&event) => BodySeed::Skip,
            _ => BodySeed::Raw,
        }
    }
}

impl<'de> DeserializeSeed<'de> for BodySeed {
    type Value = Body;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Body, D::Error> {
        Ok(match self {
            BodySeed::Raw => Body::Raw(Deserialize::deserialize(deserializer)?),
            BodySeed::Ready => Body::Ready(Box::new(Ready::deserialize(deserializer)?)),
            BodySeed::Skip => {
                IgnoredAny::deserialize(deserializer)?;
                Body::Skipped
            }
        })
    }
}

struct FrameSeed<'a>(&'a mut Option<String>);

impl<'de> DeserializeSeed<'de> for FrameSeed<'_> {
    type Value = Frame;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Frame, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FrameSeed<'_> {
    type Value = Frame;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a gateway message")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Frame, A::Error> {
        let event = self.0;
        let mut op = None;
        let mut s = None;
        let mut body = Body::Missing;
        while let Some(key) = map.next_key()? {
            match key {
                Key::Op => op = Some(map.next_value()?),
                Key::S => s = map.next_value()?,
                Key::T => *event = map.next_value()?,
                Key::D => body = map.next_value_seed(BodySeed::pick(op, event.as_deref()))?,
                Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let op = op.ok_or_else(|| de::Error::missing_field("op"))?;
        Ok(Frame { op, s, body })
    }
}

fn read_frame<'de, R: serde_json::de::Read<'de>>(
    de: &mut serde_json::Deserializer<R>,
    event: &mut Option<String>,
) -> serde_json::Result<Frame> {
    let frame = FrameSeed(event).deserialize(&mut *de)?;
    de.end()?;
    Ok(frame)
}

struct Inflater<'a> {
    zlib: &'a mut flate2::Decompress,
    input: &'a [u8],
}

impl io::Read for Inflater<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let before_in = self.zlib.total_in();
            let before_out = self.zlib.total_out();
            self.zlib
                .decompress(self.input, buf, flate2::FlushDecompress::Sync)
                .map_err(io::Error::other)?;
            let used = (self.zlib.total_in() - before_in) as usize;
            let produced = (self.zlib.total_out() - before_out) as usize;
            self.input = &self.input[used..];
            if produced > 0 || used == 0 || buf.is_empty() {
                return Ok(produced);
            }
        }
    }
}

#[derive(Deserialize)]
struct Hello {
    heartbeat_interval: u64,
}

#[derive(Deserialize)]
struct Ready {
    session_id: String,
    #[serde(default)]
    resume_gateway_url: Option<String>,
    #[serde(default)]
    user: User,
    #[serde(default)]
    sessions: Option<Vec<SessionPayload>>,
    #[serde(default)]
    user_settings_proto: Option<String>,
}

#[derive(Deserialize)]
struct SettingsBody {
    #[serde(rename = "type")]
    kind: u64,
    proto: String,
}

#[derive(Deserialize)]
struct SettingsUpdate {
    settings: SettingsBody,
    #[serde(default)]
    partial: bool,
}

pub fn gateway_url(base: &str) -> String {
    let base = base.split('?').next().unwrap_or(base);
    let after_scheme = base.find("://").map(|i| i + 3).unwrap_or(0);
    if base[after_scheme..].contains('/') {
        format!("{}{}", base, GATEWAY_QUERY)
    } else {
        format!("{}/{}", base, GATEWAY_QUERY)
    }
}

pub fn heartbeat(seq: Option<u64>) -> String {
    match seq {
        Some(seq) => format!("{{\"op\":1,\"d\":{}}}", seq),
        None => "{\"op\":1,\"d\":null}".into(),
    }
}

const VOICE_STATE: &str =
    "{\"op\":4,\"d\":{\"guild_id\":null,\"channel_id\":null,\"self_mute\":false,\"self_deaf\":false,\"self_video\":false}}";

enum Step {
    Nothing,
    Send(String),
    Heartbeat(u64),
    End(End, Option<u16>),
}

pub struct Gateway {
    http: Rc<Http>,
    link: Link,
    notes: UnboundedSender<Note>,
    commands: UnboundedReceiver<Command>,
    seq: Option<u64>,
    session_id: Option<String>,
    resume_url: Option<String>,
    zlib: flate2::Decompress,
    pending: Vec<u8>,
}

impl Gateway {
    pub fn new(http: Rc<Http>, link: Link, notes: UnboundedSender<Note>, commands: UnboundedReceiver<Command>) -> Self {
        Gateway {
            http,
            link,
            notes,
            commands,
            seq: None,
            session_id: None,
            resume_url: None,
            zlib: flate2::Decompress::new(true),
            pending: Vec::new(),
        }
    }

    pub async fn run(mut self) {
        let result = self.lifetime().await;
        self.link.borrow_mut().connected = false;
        let _ = self.notes.send(Note::Ended(result));
    }

    fn note(&self, note: Note) {
        let _ = self.notes.send(note);
    }

    async fn closed(commands: &mut UnboundedReceiver<Command>) {
        while let Some(command) = commands.recv().await {
            if let Command::Close = command {
                return;
            }
        }
    }

    fn drain(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            if let Command::Close = command {
                return true;
            }
        }
        false
    }

    async fn wait(&mut self, seconds: f64) -> bool {
        tokio::select! {
            _ = Self::closed(&mut self.commands) => true,
            _ = sleep(Duration::from_secs_f64(seconds)) => false,
        }
    }

    async fn lifetime(&mut self) -> Result<(), Error> {
        let http = self.http.clone();
        let user = tokio::select! {
            _ = Self::closed(&mut self.commands) => return Ok(()),
            user = async { http.startup().await; http.login().await } => user?,
        };
        self.link.borrow_mut().user = user.display();
        let mut backoff = Backoff::new();
        let mut resume = false;
        loop {
            if self.drain() {
                return Ok(());
            }
            let resuming = resume && self.session_id.is_some();
            let base = match (&self.resume_url, resuming) {
                (Some(url), true) => url.clone(),
                _ => GATEWAY.to_string(),
            };
            let hello = if resuming {
                self.resume_payload()
            } else {
                self.identify_payload()
            };
            let connecting = tokio::select! {
                _ = Self::closed(&mut self.commands) => return Ok(()),
                result = timeout(CONNECT_TIMEOUT, open(&http, gateway_url(&base), hello)) => result,
            };
            let end = match connecting {
                Ok(Ok(ws)) => {
                    self.fresh_connection();
                    self.link.borrow_mut().connected = true;
                    let end = self.session(ws).await;
                    self.link.borrow_mut().connected = false;
                    end
                }
                Ok(Err(e)) => End::Failed(e),
                Err(_) => End::Failed(format!("timed out after {}s", CONNECT_TIMEOUT.as_secs())),
            };
            self.note(Note::Disconnected);
            match end {
                End::Closed => return Ok(()),
                End::Reconnect { resume: again } => resume = again,
                End::Fatal(code) => {
                    return Err(Error::new(format!(
                        "the gateway closed the connection with code {}",
                        code
                    )))
                }
                End::Failed(reason) => {
                    let delay = backoff.delay();
                    warning!("gateway connection failed ({}), reconnecting in {:.2}s", reason, delay);
                    if self.wait(delay).await {
                        return Ok(());
                    }
                    resume = true;
                }
            }
        }
    }

    fn fresh_connection(&mut self) {
        self.zlib.reset(true);
        self.pending.clear();
        let mut shared = self.link.borrow_mut();
        shared.state.ws_status = UNKNOWN.into();
        shared.state.ws_activities.clear();
    }

    fn identify_payload(&self) -> String {
        let (status, activities) = self.link.borrow().state.identify_presence();
        let presence = object(&[
            ("status", json!(status)),
            ("since", json!(0)),
            ("activities", presence_activities(&activities)),
            ("afk", json!(true)),
        ]);
        format!(
            "{{\"op\":2,\"d\":{{\"token\":{},\"capabilities\":{},\"properties\":{},\"presence\":{},\"compress\":false,\"client_state\":{{\"guild_versions\":{{}}}}}}}}",
            Value::from(self.http.token.as_str()),
            CAPABILITIES,
            self.http.properties(),
            presence
        )
    }

    fn resume_payload(&self) -> String {
        let d = object(&[
            ("seq", json!(self.seq)),
            ("session_id", json!(self.session_id)),
            ("token", json!(self.http.token)),
        ]);
        format!("{{\"op\":6,\"d\":{}}}", d)
    }

    async fn session(&mut self, mut ws: WebSocket) -> End {
        let mut last_recv = Instant::now();
        let mut beat: Option<(Duration, Instant)> = None;
        let end = loop {
            let next_beat = beat.map(|(_, at)| at).unwrap_or_else(Instant::now);
            let step = tokio::select! {
                message = ws.next() => {
                    last_recv = Instant::now();
                    match message {
                        Some(Ok(Message::Binary(bytes))) => self.inflate(&bytes),
                        Some(Ok(Message::Text(text))) => self.handle(text.as_str()),
                        Some(Ok(Message::Close(frame))) => {
                            let code = frame.map(|f| u16::from(f.code));
                            Step::End(closed_by_server(code), None)
                        }
                        Some(Ok(_)) => Step::Nothing,
                        Some(Err(e)) => {
                            warning!("gateway read failed: {}", e);
                            Step::End(End::Reconnect { resume: true }, None)
                        }
                        None => Step::End(End::Reconnect { resume: true }, None),
                    }
                }
                command = self.commands.recv() => match command {
                    Some(Command::Send(payload)) => Step::Send(payload),
                    Some(Command::Close) | None => Step::End(End::Closed, Some(1000)),
                },
                _ = sleep_until(next_beat), if beat.is_some() => {
                    if let Some((interval, _)) = beat {
                        beat = Some((interval, Instant::now() + interval));
                    }
                    Step::Send(heartbeat(self.seq))
                }
                _ = sleep_until(last_recv + RECEIVE_TIMEOUT) => {
                    warning!("the gateway stopped responding, reconnecting");
                    Step::End(End::Reconnect { resume: true }, Some(4000))
                }
            };
            match step {
                Step::Nothing => {}
                Step::Send(payload) => {
                    if let Err(e) = ws.send(Message::text(payload)).await {
                        warning!("gateway write failed: {}", e);
                        break End::Reconnect { resume: true };
                    }
                }
                Step::Heartbeat(interval) => {
                    let interval = Duration::from_millis(interval);
                    beat = Some((interval, Instant::now() + interval));
                    if let Err(e) = ws.send(Message::text(heartbeat(self.seq))).await {
                        warning!("gateway write failed: {}", e);
                        break End::Reconnect { resume: true };
                    }
                }
                Step::End(end, code) => {
                    if let Some(code) = code {
                        let _ = timeout(CLOSE_TIMEOUT, ws.close(code, "")).await;
                    }
                    break end;
                }
            }
        };
        self.pending = Vec::new();
        end
    }

    fn inflate(&mut self, bytes: &[u8]) -> Step {
        if self.pending.is_empty() && bytes.ends_with(&ZLIB_SUFFIX) {
            return self.decode(bytes);
        }
        self.pending.extend_from_slice(bytes);
        if !self.pending.ends_with(&ZLIB_SUFFIX) {
            return Step::Nothing;
        }
        let mut input = std::mem::take(&mut self.pending);
        let step = self.decode(&input);
        input.clear();
        if input.capacity() > BUFFER_KEEP {
            input.shrink_to(BUFFER_KEEP);
        }
        self.pending = input;
        step
    }

    fn decode(&mut self, input: &[u8]) -> Step {
        let mut event = None;
        let mut reader = BufReader::with_capacity(
            INFLATE_CHUNK,
            Inflater {
                zlib: &mut self.zlib,
                input,
            },
        );
        let parsed = read_frame(&mut serde_json::Deserializer::from_reader(&mut reader), &mut event);
        let drained = io::copy(&mut reader, &mut io::sink());
        let parsed = match (parsed, drained) {
            (Ok(_), Err(e)) => Err(serde_json::Error::io(e)),
            (parsed, _) => parsed,
        };
        self.apply(parsed, event)
    }

    fn handle(&mut self, text: &str) -> Step {
        let mut event = None;
        let parsed = read_frame(&mut serde_json::Deserializer::from_str(text), &mut event);
        self.apply(parsed, event)
    }

    fn apply(&mut self, parsed: serde_json::Result<Frame>, event: Option<String>) -> Step {
        let frame = match parsed {
            Ok(frame) => frame,
            Err(e) if e.is_io() => {
                warning!("could not inflate a gateway message: {}", e);
                return Step::End(End::Reconnect { resume: true }, Some(4000));
            }
            Err(e) if event.as_deref() == Some("READY") => {
                warning!("could not read READY: {}", e);
                return Step::End(End::Failed(format!("unreadable READY: {}", e)), Some(4000));
            }
            Err(e) => {
                warning!("could not read a gateway message: {}", e);
                return Step::Nothing;
            }
        };
        if let Some(seq) = frame.s {
            self.seq = Some(seq);
        }
        let data = frame.body.text();
        match frame.op {
            0 => self.dispatch(event.as_deref().unwrap_or(""), frame.body),
            1 => Step::Send(heartbeat(self.seq)),
            7 => {
                info!("the gateway asked for a reconnect");
                Step::End(End::Reconnect { resume: true }, Some(4000))
            }
            9 => {
                if data.trim() == "true" {
                    return Step::End(End::Reconnect { resume: true }, Some(4000));
                }
                info!("the gateway session was invalidated, identifying again");
                self.seq = None;
                self.session_id = None;
                self.resume_url = None;
                Step::End(End::Reconnect { resume: false }, Some(1000))
            }
            10 => match serde_json::from_str::<Hello>(data) {
                Ok(hello) => Step::Heartbeat(hello.heartbeat_interval),
                Err(e) => {
                    warning!("could not read the gateway hello: {}", e);
                    Step::End(End::Reconnect { resume: true }, Some(4000))
                }
            },
            _ => Step::Nothing,
        }
    }

    fn dispatch(&mut self, event: &str, body: Body) -> Step {
        let data = body.text();
        match event {
            "READY" => match body.into_ready() {
                Ok(ready) => {
                    self.session_id = Some(ready.session_id.clone());
                    self.resume_url = ready.resume_gateway_url.clone();
                    let mut shared = self.link.borrow_mut();
                    shared.user = ready.user.display();
                    shared.state.ready(
                        ready.session_id,
                        Some(ready.sessions.unwrap_or_default()),
                        ready.user_settings_proto.as_deref(),
                    );
                    Step::Send(VOICE_STATE.into())
                }
                Err(e) => {
                    warning!("could not read READY: {}", e);
                    Step::End(End::Failed(format!("unreadable READY: {}", e)), Some(4000))
                }
            },
            "READY_SUPPLEMENTAL" => {
                self.note(Note::Ready);
                Step::Nothing
            }
            "RESUMED" => {
                self.note(Note::Resumed);
                Step::Nothing
            }
            "SESSIONS_REPLACE" => {
                match serde_json::from_str::<Vec<SessionPayload>>(data) {
                    Ok(sessions) => {
                        if self.link.borrow_mut().state.sessions_replace(&sessions, false) {
                            self.note(Note::Sessions);
                        }
                    }
                    Err(e) => warning!("could not read SESSIONS_REPLACE: {}", e),
                }
                Step::Nothing
            }
            "USER_SETTINGS_PROTO_UPDATE" => {
                match serde_json::from_str::<SettingsUpdate>(data) {
                    Ok(update) => {
                        let change = self.link.borrow_mut().state.settings_update(
                            update.settings.kind,
                            &update.settings.proto,
                            update.partial,
                        );
                        if let Some((after, before)) = change {
                            self.note(Note::Settings { after, before });
                        }
                    }
                    Err(e) => warning!("could not read USER_SETTINGS_PROTO_UPDATE: {}", e),
                }
                Step::Nothing
            }
            "USER_UPDATE" => {
                if let Ok(user) = serde_json::from_str::<User>(data) {
                    if !user.username.is_empty() {
                        self.link.borrow_mut().user = user.display();
                    }
                }
                Step::Nothing
            }
            other => {
                self.link.borrow_mut().state.note_ignored(other);
                Step::Nothing
            }
        }
    }
}

async fn open(http: &Http, url: String, hello: String) -> Result<WebSocket, String> {
    let response = http
        .client
        .websocket(url)
        .emulation(emulation(false, false))
        .default_headers(false)
        .orig_headers(order(&WS_ORDER))
        .header("User-Agent", user_agent(CHROME))
        .header("Origin", "https://discord.com")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept-Encoding", "gzip, deflate, br, zstd")
        .header("Sec-WebSocket-Extensions", "permessage-deflate; client_max_window_bits")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if status != 101 {
        return Err(format!("the gateway answered HTTP {}", status));
    }
    let mut ws = response.into_websocket().await.map_err(|e| e.to_string())?;
    ws.send(Message::text(hello)).await.map_err(|e| e.to_string())?;
    Ok(ws)
}

fn closed_by_server(code: Option<u16>) -> End {
    match code {
        Some(code) if FATAL_CLOSES.contains(&code) || code == INTENTS_CLOSE => End::Fatal(code),
        _ => End::Reconnect { resume: true },
    }
}

pub struct GatewaySession {
    pub gateway: Option<Gateway>,
    pub commands: UnboundedSender<Command>,
    pub link: Link,
    pub sessions_only: bool,
    pub ready_file: Option<PathBuf>,
    pub watchdog: Option<(Arc<AtomicU64>, u64)>,
}

impl runner::Session for GatewaySession {
    fn start(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            tokio::task::spawn_local(gateway.run());
        }
    }

    fn close(&mut self) {
        let _ = self.commands.send(Command::Close);
    }

    fn on_ready(&mut self, first: bool) -> bool {
        {
            let shared = self.link.borrow();
            info!("connected as {}", shared.user);
            for line in shared.state.describe_sessions() {
                info!("{}", line);
            }
        }
        if let Some(ready_file) = &self.ready_file {
            write_atomic(ready_file, "ok");
        }
        if self.sessions_only {
            return true;
        }
        if first {
            if let Some((beat, seconds)) = self.watchdog.take() {
                Watchdog {
                    last_tick: Box::new(beat_reader(beat)),
                    timeout: seconds as f64,
                    action: "exiting",
                    on_stall: Box::new(|| std::process::exit(1)),
                    clock: Box::new(crate::controller::seconds),
                    fired: false,
                }
                .start((seconds as f64 / 3.0).max(1.0));
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn gateway() -> (Gateway, UnboundedReceiver<Note>, UnboundedSender<Command>) {
        let (notes_tx, notes_rx) = unbounded_channel();
        let (commands_tx, commands_rx) = unbounded_channel();
        let http = Rc::new(Http::new("token".into()).unwrap());
        (Gateway::new(http, link(), notes_tx, commands_rx), notes_rx, commands_tx)
    }

    fn notes(rx: &mut UnboundedReceiver<Note>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(note) = rx.try_recv() {
            out.push(format!("{:?}", note));
        }
        out
    }

    fn settings(status: &str) -> String {
        proto::b64encode(&proto::status_patch(&[], status))
    }

    #[test]
    fn sec_ch_ua_matches_the_library_shuffle() {
        assert_eq!(
            sec_ch_ua(149),
            "\"Google Chrome\";v=\"149\", \"Chromium\";v=\"149\", \"Not)A;Brand\";v=\"24\""
        );
        assert_eq!(
            sec_ch_ua(132),
            "\"Not A(Brand\";v=\"8\", \"Chromium\";v=\"132\", \"Google Chrome\";v=\"132\""
        );
    }

    #[test]
    fn super_properties_keep_their_order() {
        let base = super_properties(149, 123, "L", "H");
        let encoded = object(&base);
        assert!(encoded
            .starts_with("{\"os\":\"Windows\",\"browser\":\"Chrome\",\"device\":\"\",\"system_locale\":\"en-US\""));
        assert!(encoded.contains("\"client_build_number\":123,\"client_event_source\":null,\"has_client_mods\":false"));
        assert!(encoded.ends_with("\"client_app_state\":\"unfocused\",\"client_heartbeat_session_id\":\"H\"}"));
        let props = gateway_properties(base);
        assert!(props.ends_with(
            "\"is_fast_connect\":false,\"latest_headless_tasks\":[],\"latest_headless_task_run_seconds_before\":null,\"gateway_connect_reasons\":\"AppSkeleton\"}"
        ));
        let value: Value = serde_json::from_str(&props).unwrap();
        assert_eq!(value["browser_user_agent"], user_agent(149));
    }

    #[test]
    fn finds_the_build_number() {
        let page = "<script src=\"/assets/web.abc.js\"></script><script src=\"/assets/sentry.f00d_12.js\" defer>";
        assert_eq!(find_sentry(page), Some("sentry.f00d_12"));
        assert_eq!(find_sentry("assets/sentry..js nothing"), None);
        let script = "x={release:\"discord\",buildNumber:\"412345\",y:1}";
        assert_eq!(find_build_number(script), Some(412345));
        assert_eq!(find_build_number("buildNumber:12,buildNumber:\"77\""), Some(77));
        assert_eq!(find_build_number("nothing here"), None);
    }

    #[test]
    fn uuids_look_like_version_four() {
        let id = uuid4();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(uuid4(), uuid4());
    }

    #[test]
    fn backoff_grows_then_caps() {
        let mut backoff = Backoff::new();
        for expected in [2.0, 4.0, 8.0] {
            let delay = backoff.delay();
            assert!((0.0..=expected).contains(&delay));
        }
        for _ in 0..20 {
            assert!(backoff.delay() <= 1024.0);
        }
        assert_eq!(backoff.exp, 10);
    }

    #[test]
    fn gateway_urls_carry_the_query() {
        assert_eq!(
            gateway_url(GATEWAY),
            "wss://gateway.discord.gg/?v=9&encoding=json&compress=zlib-stream"
        );
        assert_eq!(
            gateway_url("wss://gateway-us-east1-b.discord.gg"),
            "wss://gateway-us-east1-b.discord.gg/?v=9&encoding=json&compress=zlib-stream"
        );
    }

    #[test]
    fn presence_and_heartbeat_payloads() {
        assert_eq!(heartbeat(None), "{\"op\":1,\"d\":null}");
        assert_eq!(heartbeat(Some(42)), "{\"op\":1,\"d\":42}");
        assert_eq!(
            presence_payload("idle", &[]),
            "{\"op\":3,\"d\":{\"activities\":[],\"afk\":true,\"since\":0,\"status\":\"idle\"}}"
        );
        let custom = json!({"type": 4, "state": "hi", "name": "Custom Status", "created_at": 1, "id": "custom"});
        let payload: Value = serde_json::from_str(&presence_payload("online", &[custom])).unwrap();
        assert_eq!(
            payload["d"]["activities"][0],
            json!({"type": 4, "state": "hi", "name": "Custom Status"})
        );
    }

    #[test]
    fn identify_and_resume_payloads() {
        let (mut gw, _, _) = gateway();
        let identify: Value = serde_json::from_str(&gw.identify_payload()).unwrap();
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], "token");
        assert_eq!(identify["d"]["capabilities"], 22525);
        assert_eq!(
            identify["d"]["presence"],
            json!({"status": "unknown", "since": 0, "activities": [], "afk": true})
        );
        assert_eq!(identify["d"]["properties"]["client_build_number"], 9999);
        assert_eq!(identify["d"]["compress"], false);
        assert_eq!(identify["d"]["client_state"], json!({"guild_versions": {}}));
        gw.seq = Some(7);
        gw.session_id = Some("abc".into());
        assert_eq!(
            gw.resume_payload(),
            "{\"op\":6,\"d\":{\"seq\":7,\"session_id\":\"abc\",\"token\":\"token\"}}"
        );
    }

    fn ready_json(sessions: &str, settings_status: &str) -> String {
        format!(
            "{{\"op\":0,\"s\":1,\"t\":\"READY\",\"d\":{{\"v\":9,\"guilds\":[{{\"id\":\"1\"}}],\"session_id\":\"me\",\"resume_gateway_url\":\"wss://r.discord.gg\",\"user\":{{\"username\":\"someone\",\"discriminator\":\"0\"}},\"sessions\":{},\"user_settings_proto\":\"{}\"}}}}",
            sessions,
            settings(settings_status)
        )
    }

    #[test]
    fn ready_and_supplemental_drive_the_runner_notes() {
        let (mut gw, mut rx, _) = gateway();
        let sessions = r#"[{"session_id":"me","client_info":{"client":"web"},"status":"online","activities":[]},
                           {"session_id":"p","client_info":{"client":"mobile"},"status":"online","activities":[]}]"#;
        let step = gw.handle(&ready_json(sessions, "dnd"));
        assert!(matches!(step, Step::Send(ref s) if s == VOICE_STATE));
        assert_eq!(gw.seq, Some(1));
        assert_eq!(gw.session_id.as_deref(), Some("me"));
        assert_eq!(gw.resume_url.as_deref(), Some("wss://r.discord.gg"));
        assert!(notes(&mut rx).is_empty());
        {
            let shared = gw.link.borrow();
            assert_eq!(shared.user, "someone");
            assert!(shared.state.is_on_mobile());
            assert_eq!(shared.state.saved_status(), "dnd");
        }
        gw.handle(r#"{"op":0,"s":2,"t":"READY_SUPPLEMENTAL","d":{"guilds":[]}}"#);
        assert_eq!(notes(&mut rx), vec!["Ready"]);
    }

    #[test]
    fn session_and_settings_events_become_notes() {
        let (mut gw, mut rx, _) = gateway();
        gw.handle(&ready_json(
            r#"[{"session_id":"me","client_info":{"client":"web"},"status":"online"}]"#,
            "online",
        ));
        gw.handle(
            r#"{"op":0,"s":3,"t":"SESSIONS_REPLACE","d":[{"session_id":"me","client_info":{"client":"web"},"status":"online"},
               {"session_id":"p","client_info":{"client":"mobile"},"status":"online"}]}"#,
        );
        gw.handle(&format!(
            "{{\"op\":0,\"s\":4,\"t\":\"USER_SETTINGS_PROTO_UPDATE\",\"d\":{{\"settings\":{{\"type\":1,\"proto\":\"{}\"}},\"partial\":true}}}}",
            settings("idle")
        ));
        gw.handle(r#"{"op":0,"s":5,"t":"RESUMED","d":null}"#);
        gw.handle(r#"{"op":0,"s":6,"t":"TYPING_START","d":{}}"#);
        gw.handle(r#"{"op":0,"s":7,"t":"TYPING_START","d":{}}"#);
        gw.handle(r#"{"op":0,"s":8,"t":"MESSAGE_CREATE","d":{}}"#);
        assert_eq!(
            notes(&mut rx),
            vec![
                "Sessions",
                "Settings { after: \"idle\", before: \"online\" }",
                "Resumed"
            ]
        );
        assert_eq!(gw.seq, Some(8));
        assert_eq!(
            gw.link.borrow().state.ignored_summary(8),
            "TYPING_START=2 MESSAGE_CREATE=1"
        );
    }

    #[test]
    fn control_opcodes() {
        let (mut gw, _, _) = gateway();
        gw.seq = Some(5);
        gw.session_id = Some("s".into());
        gw.resume_url = Some("wss://r".into());
        assert!(matches!(
            gw.handle(r#"{"op":10,"d":{"heartbeat_interval":41250}}"#),
            Step::Heartbeat(41250)
        ));
        assert!(matches!(gw.handle(r#"{"op":1,"d":null}"#), Step::Send(ref s) if s == "{\"op\":1,\"d\":5}"));
        assert!(matches!(gw.handle(r#"{"op":11}"#), Step::Nothing));
        assert!(matches!(
            gw.handle(r#"{"op":7,"d":null}"#),
            Step::End(End::Reconnect { resume: true }, Some(4000))
        ));
        assert!(matches!(
            gw.handle(r#"{"op":9,"d":true}"#),
            Step::End(End::Reconnect { resume: true }, Some(4000))
        ));
        assert_eq!(gw.session_id.as_deref(), Some("s"));
        assert!(matches!(
            gw.handle(r#"{"op":9,"d":false}"#),
            Step::End(End::Reconnect { resume: false }, Some(1000))
        ));
        assert_eq!(gw.session_id, None);
        assert_eq!(gw.seq, None);
        assert_eq!(gw.resume_url, None);
    }

    #[test]
    fn server_closes_map_like_the_library() {
        assert_eq!(closed_by_server(Some(4000)), End::Reconnect { resume: true });
        assert_eq!(closed_by_server(Some(1000)), End::Reconnect { resume: true });
        assert_eq!(closed_by_server(None), End::Reconnect { resume: true });
        assert_eq!(closed_by_server(Some(4004)), End::Fatal(4004));
        assert_eq!(closed_by_server(Some(4014)), End::Fatal(4014));
    }

    #[test]
    fn zlib_stream_frames_inflate_across_chunks() {
        use flate2::{Compress, Compression, FlushCompress};
        let (mut gw, mut rx, _) = gateway();
        let mut z = Compress::new(Compression::default(), true);
        let mut compress = |text: &str| {
            let mut out = Vec::with_capacity(text.len() + 64);
            z.compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync).unwrap();
            out
        };
        let big_sessions: String = (0..400)
            .map(|i| {
                format!(
                    "{{\"session_id\":\"s{}\",\"client_info\":{{\"client\":\"desktop\"}},\"status\":\"online\"}}",
                    i
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let ready = compress(&ready_json(&format!("[{}]", big_sessions), "online"));
        let (a, b) = ready.split_at(ready.len() / 2);
        assert!(matches!(gw.inflate(a), Step::Nothing));
        assert!(matches!(gw.inflate(b), Step::Send(_)));
        assert_eq!(gw.link.borrow().state.sessions.len(), 401);
        let supplemental = compress(r#"{"op":0,"s":2,"t":"READY_SUPPLEMENTAL","d":{}}"#);
        assert!(matches!(gw.inflate(&supplemental), Step::Nothing));
        assert_eq!(notes(&mut rx), vec!["Ready"]);
        assert_eq!(gw.seq, Some(2));
    }

    #[test]
    fn frames_parse_in_any_key_order() {
        let (mut gw, mut rx, _) = gateway();
        let sessions = r#"[{"session_id":"me","client_info":{"client":"web"},"status":"online"}]"#;
        let ready = format!(
            "{{\"t\":\"READY\",\"s\":3,\"op\":0,\"d\":{{\"guilds\":[{{\"channels\":[1,2,{{\"x\":\"y\"}}]}}],\"session_id\":\"me\",\"user\":{{\"username\":\"someone\",\"discriminator\":\"0\"}},\"sessions\":{}}}}}",
            sessions
        );
        assert!(matches!(gw.handle(&ready), Step::Send(ref s) if s == VOICE_STATE));
        assert_eq!(gw.session_id.as_deref(), Some("me"));
        assert_eq!(gw.seq, Some(3));
        let late = format!(
            "{{\"d\":{{\"session_id\":\"again\",\"sessions\":{}}},\"op\":0,\"s\":4,\"t\":\"READY\"}}",
            sessions
        );
        assert!(matches!(gw.handle(&late), Step::Send(_)));
        assert_eq!(gw.session_id.as_deref(), Some("again"));
        gw.handle(r#"{"d":{},"op":0,"s":5,"t":"GUILD_CREATE"}"#);
        gw.handle(r#"{"t":"GUILD_CREATE","s":6,"op":0,"d":{"huge":[1,2,3]},"extra":true}"#);
        assert_eq!(gw.link.borrow().state.ignored_summary(8), "GUILD_CREATE=2");
        assert_eq!(gw.seq, Some(6));
        assert!(notes(&mut rx).is_empty());
    }

    #[test]
    fn unreadable_ready_fails_the_connection() {
        let (mut gw, _rx, _) = gateway();
        let step = gw.handle(r#"{"t":"READY","s":1,"op":0,"d":{"user":{}}}"#);
        assert!(matches!(step, Step::End(End::Failed(_), Some(4000))));
        assert!(matches!(gw.handle(r#"{"op":0,"t":"X","d":[}"#), Step::Nothing));
    }

    #[test]
    fn zlib_survives_a_bad_body_and_reconnects_on_corruption() {
        use flate2::{Compress, Compression, FlushCompress};
        let (mut gw, _rx, _) = gateway();
        let mut z = Compress::new(Compression::default(), true);
        let mut compress = |text: &str| {
            let mut out = Vec::with_capacity(text.len() + 64);
            z.compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync).unwrap();
            out
        };
        let filler: String = (0..20_000).map(|i| format!("{},", i)).collect();
        let bad = compress(&format!("{{\"op\":0,\"s\":1,\"t\":\"USER_UPDATE\",\"d\":[{}}}", filler));
        assert!(matches!(gw.inflate(&bad), Step::Nothing));
        let hello = compress(r#"{"t":null,"s":null,"op":10,"d":{"heartbeat_interval":41250}}"#);
        assert!(matches!(gw.inflate(&hello), Step::Heartbeat(41250)));
        let mut corrupt = vec![0xffu8; 16];
        corrupt.extend_from_slice(&ZLIB_SUFFIX);
        assert!(matches!(
            gw.inflate(&corrupt),
            Step::End(End::Reconnect { resume: true }, Some(4000))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn presence_needs_a_connection_and_queues_op3() {
        let (commands_tx, mut commands_rx) = unbounded_channel();
        let shared = link();
        let client = DiscordClient {
            link: shared.clone(),
            http: Rc::new(Http::new("t".into()).unwrap()),
            commands: commands_tx,
        };
        use crate::runner::Client;
        assert!(client.hide().await.is_err());
        shared.borrow_mut().connected = true;
        client.hide().await.unwrap();
        client.unhide("offline").await.unwrap();
        let sent: Vec<String> = std::iter::from_fn(|| match commands_rx.try_recv() {
            Ok(Command::Send(s)) => Some(s),
            _ => None,
        })
        .collect();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].contains("\"status\":\"invisible\""));
        assert!(sent[1].contains("\"status\":\"invisible\""));
        assert_eq!(shared.borrow().state.ws_status, "invisible");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn presence_after_a_resume_keeps_our_activities() {
        let (commands_tx, mut commands_rx) = unbounded_channel();
        let shared = link();
        let client = DiscordClient {
            link: shared.clone(),
            http: Rc::new(Http::new("t".into()).unwrap()),
            commands: commands_tx,
        };
        {
            let mut s = shared.borrow_mut();
            let ours: SessionPayload = serde_json::from_value(json!({
                "session_id": "me", "client_info": {"client": "web"}, "status": "online",
                "activities": [{"type": 4, "state": "busy", "name": "Custom Status", "id": "custom"}]
            }))
            .unwrap();
            s.state.ready("me".into(), Some(vec![ours]), None);
            s.state.ws_status = UNKNOWN.into();
            s.state.ws_activities.clear();
            s.connected = true;
        }
        use crate::runner::Client;
        client.hide().await.unwrap();
        client.unhide("online").await.unwrap();
        let sent: Vec<String> = std::iter::from_fn(|| match commands_rx.try_recv() {
            Ok(Command::Send(s)) => Some(s),
            _ => None,
        })
        .collect();
        assert_eq!(sent.len(), 2);
        for payload in &sent {
            assert!(payload.contains("\"activities\":[{\"name\":\"Custom Status\",\"state\":\"busy\",\"type\":4}]"));
        }
    }
}
