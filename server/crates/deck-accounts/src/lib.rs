//! Claude Code accounts of one deck-server machine (`deck.accounts.*`).
//!
//! The same job claude-swap (`cswap`) does on a desktop, done by the server
//! for its own machine, so every machine Deck reaches keeps its own
//! accounts and its headless agents run under that machine's active one:
//!
//! * the store: one slot per account under the server's app dir
//!   (`accounts/accounts.json` and `accounts/slot-N.json` holding the
//!   account's `claudeAiOauth` credentials and its `oauthAccount` identity);
//! * the live login: Claude Code's own `.credentials.json` (in
//!   `CLAUDE_CONFIG_DIR` or `~/.claude`) and the `oauthAccount` in its global
//!   config (`~/.claude.json`). Switching writes the chosen slot there,
//!   after saving the live (possibly refreshed) tokens of the account it
//!   leaves; an account signed in by other means is adopted into the store;
//! * usage: the 5-hour / 7-day / per-model windows from Anthropic's usage
//!   endpoint, in the shape `cswap list --json` prints;
//! * sign-in: Claude Code's OAuth flow with a pasted code (the "manual"
//!   redirect it uses when no browser can call back): [`Accounts::login_start`]
//!   gives the URL to open, [`Accounts::login_finish`] exchanges the code
//!   the page shows and stores the account. Credentials never leave the
//!   machine they were issued for.
//!
//! HTTP goes through [`Http`] ([`Curl`] in the server, a fake in tests).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

mod http;
pub use http::{Curl, Http, HttpRequest, HttpResponse};

// ---- Claude Code's OAuth client (as the installed CLI uses it) ---------------

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// The redirect for a pasted code: the page shows `CODE#STATE`.
pub const MANUAL_REDIRECT_URL: &str = "https://platform.claude.com/oauth/code/callback";
pub const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
/// What Claude Code asks for when it signs in with a Claude subscription.
pub const SCOPES: &[&str] = &[
    "org:create_api_key",
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
    "user:plugins",
];

/// How long usage numbers are reused before the endpoint is asked again
/// (it rate-limits; Deck asks every 15 s).
const USAGE_TTL: Duration = Duration::from_secs(180);
/// A sign-in that was started but not finished is forgotten after this.
const LOGIN_TTL: Duration = Duration::from_secs(30 * 60);
/// Tokens this close to expiry count as expired.
const EXPIRY_MARGIN_MS: u64 = 5 * 60 * 1000;

pub type Result<T> = std::result::Result<T, String>;

/// Where the store and Claude Code's live login are.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The store (`<server app dir>/accounts`).
    pub store: PathBuf,
    /// Claude Code's config home (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude_home: PathBuf,
    /// Claude Code's global config (`.claude.json`).
    pub global_config: PathBuf,
}

impl Paths {
    /// Claude Code's own resolution: `CLAUDE_CONFIG_DIR` if set, else
    /// `~/.claude`; the global config is `<home>/.config.json` if that
    /// legacy file exists, else `(CLAUDE_CONFIG_DIR || ~)/.claude.json`.
    pub fn from_env(store: PathBuf) -> Paths {
        let user_home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        let claude_home = config_dir
            .clone()
            .unwrap_or_else(|| user_home.join(".claude"));
        let legacy = claude_home.join(".config.json");
        let global_config = if legacy.is_file() {
            legacy
        } else {
            config_dir.unwrap_or(user_home).join(".claude.json")
        };
        Paths {
            store,
            claude_home,
            global_config,
        }
    }

    pub fn credentials(&self) -> PathBuf {
        self.claude_home.join(".credentials.json")
    }
}

// ---- small helpers -------------------------------------------------------------

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_token(len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|e| format!("no randomness: {e}"))?;
    Ok(b64url(&bytes))
}

/// PKCE S256: the challenge for a verifier.
pub fn code_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_json(path: &Path) -> Result<Option<Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Writes JSON next to `path` and renames it over, readable by the owner only.
fn write_json(path: &Path, value: &Value, private: bool) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let _ = private;
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.write_all(text.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", path.display())
    })
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

// ---- time ------------------------------------------------------------------------

/// Seconds since the epoch of an RFC 3339 time (`2026-12-01T00:59:59.74+00:00`).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', ' '])?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let (clock, offset) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, off) = rest.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let mut parts = off[1..].split(':');
        let h: i64 = parts.next()?.parse().ok()?;
        let mi: i64 = parts.next().unwrap_or("0").parse().ok()?;
        (c, sign * (h * 3600 + mi * 60))
    };
    let mut t = clock.split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: f64 = t.next().unwrap_or("0").parse().ok()?;
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec as i64 - offset)
}

/// "6d 4h", "12h 26m", "40m": the time left until `resets_at`.
pub fn countdown(resets_at: &str, now_s: i64) -> Option<String> {
    let left = (parse_rfc3339(resets_at)? - now_s).max(0);
    let (d, h, m) = (left / 86400, left % 86400 / 3600, left % 3600 / 60);
    Some(if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    })
}

// ---- usage -----------------------------------------------------------------------

/// The usage endpoint's answer in the shape `cswap list --json` prints
/// (`fiveHour`, `sevenDay`, `scoped`), countdowns computed at `now_s`.
pub fn normalize_usage(data: &Value, now_s: i64) -> Option<Value> {
    let window = |w: &Value| -> Option<Value> {
        let pct = w.get("utilization").and_then(Value::as_f64)?;
        let mut entry = json!({ "pct": pct });
        if let Some(at) = str_of(w, "resets_at") {
            entry["resetsAt"] = json!(at);
            if let Some(c) = countdown(at, now_s) {
                entry["countdown"] = json!(c);
            }
        }
        Some(entry)
    };
    let mut out = Map::new();
    if let Some(w) = data.get("five_hour").and_then(window) {
        out.insert("fiveHour".into(), w);
    }
    if let Some(w) = data.get("seven_day").and_then(window) {
        out.insert("sevenDay".into(), w);
    }
    let scoped: Vec<Value> = data
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|lim| {
            let name = lim.pointer("/scope/model/display_name")?.as_str()?;
            let pct = lim.get("percent").and_then(Value::as_f64)?;
            let mut entry = json!({ "name": name, "pct": pct });
            if let Some(at) = str_of(lim, "resets_at") {
                entry["resetsAt"] = json!(at);
                if let Some(c) = countdown(at, now_s) {
                    entry["countdown"] = json!(c);
                }
            }
            Some(entry)
        })
        .collect();
    if !scoped.is_empty() {
        out.insert("scoped".into(), Value::Array(scoped));
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// Recomputes the countdowns of stored usage for now.
fn refresh_countdowns(usage: &Value, now_s: i64) -> Value {
    let mut usage = usage.clone();
    let fix = |w: &mut Value| {
        if let Some(c) = w
            .get("resetsAt")
            .and_then(Value::as_str)
            .and_then(|at| countdown(at, now_s))
        {
            w["countdown"] = json!(c);
        }
    };
    for key in ["fiveHour", "sevenDay"] {
        if let Some(w) = usage.get_mut(key) {
            fix(w);
        }
    }
    if let Some(list) = usage.get_mut("scoped").and_then(Value::as_array_mut) {
        list.iter_mut().for_each(fix);
    }
    usage
}

// ---- credentials -------------------------------------------------------------------

/// Claude Code's `claudeAiOauth` from a token endpoint answer.
pub fn oauth_from_token(token: &Value, subscription: Option<&str>, now: u64) -> Result<Value> {
    let access = str_of(token, "access_token").ok_or("the token answer has no access_token")?;
    let expires_in = token
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    let scopes: Vec<&str> = str_of(token, "scope")
        .map(|s| s.split_whitespace().collect())
        .unwrap_or_else(|| SCOPES.to_vec());
    let mut oauth = json!({
        "accessToken": access,
        "refreshToken": str_of(token, "refresh_token"),
        "expiresAt": now + expires_in * 1000,
        "scopes": scopes,
        "subscriptionType": subscription,
    });
    if let Some(uuid) = token.pointer("/account/uuid").and_then(Value::as_str) {
        oauth["accountUuid"] = json!(uuid);
    }
    if let Some(uuid) = token.pointer("/organization/uuid").and_then(Value::as_str) {
        oauth["organizationUuid"] = json!(uuid);
    }
    Ok(oauth)
}

/// `claude_max` → `max` (what Claude Code keeps as `subscriptionType`).
pub fn subscription_of(organization_type: Option<&str>) -> Option<String> {
    let t = organization_type?;
    Some(t.strip_prefix("claude_").unwrap_or(t).to_string())
}

/// Claude Code's `oauthAccount` from the profile endpoint's answer.
pub fn account_from_profile(profile: &Value) -> Result<Value> {
    let uuid = profile
        .pointer("/account/uuid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the profile has no account id")?;
    let email = profile
        .pointer("/account/email")
        .or_else(|| profile.pointer("/account/email_address"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let org = profile.get("organization").cloned().unwrap_or(Value::Null);
    let display = profile
        .pointer("/account/display_name")
        .and_then(Value::as_str);
    Ok(json!({
        "accountUuid": uuid,
        "emailAddress": email,
        "organizationUuid": str_of(&org, "uuid"),
        "organizationName": str_of(&org, "name"),
        "displayName": display,
    }))
}

fn expired(oauth: &Value, now: u64) -> bool {
    oauth
        .get("expiresAt")
        .and_then(Value::as_u64)
        .is_some_and(|at| now + EXPIRY_MARGIN_MS >= at)
}

/// What a pasted sign-in code is: `CODE#STATE` (the callback page), the
/// callback URL itself, or a bare code.
pub fn parse_pasted_code(input: &str) -> Result<(String, Option<String>)> {
    let input = input.trim();
    if input.is_empty() {
        return Err("paste the code the sign-in page shows".into());
    }
    if let Some((_, query)) = input.split_once('?') {
        let mut code = None;
        let mut state = None;
        for pair in query.split(['&', '#']) {
            match pair.split_once('=') {
                Some(("code", v)) => code = Some(percent_decode(v)),
                Some(("state", v)) => state = Some(percent_decode(v)),
                _ => {}
            }
        }
        if let Some(code) = code.filter(|c| !c.is_empty()) {
            return Ok((code, state));
        }
    }
    match input.split_once('#') {
        Some((code, state)) if !code.is_empty() => Ok((
            code.to_string(),
            Some(state.to_string()).filter(|s| !s.is_empty()),
        )),
        Some(_) => Err("the code is empty".into()),
        None => Ok((input.to_string(), None)),
    }
}

// ---- the store ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Slot {
    number: u32,
    /// `{"claudeAiOauth": {..}, "oauthAccount": {..}}`.
    data: Value,
}

impl Slot {
    fn oauth(&self) -> &Value {
        &self.data["claudeAiOauth"]
    }
    fn account(&self) -> &Value {
        &self.data["oauthAccount"]
    }
    fn uuid(&self) -> Option<&str> {
        str_of(self.account(), "accountUuid")
    }
    fn email(&self) -> &str {
        str_of(self.account(), "emailAddress").unwrap_or_default()
    }
}

#[derive(Default)]
struct UsageCache {
    at: Option<Instant>,
    usage: Option<Value>,
    last_good: Option<Value>,
    status: String,
}

struct Pending {
    verifier: String,
    state: String,
    started: Instant,
}

/// One machine's accounts.
pub struct Accounts<H: Http> {
    paths: Paths,
    http: H,
    /// The store and the live files are read and written under this lock.
    lock: Mutex<()>,
    usage: Mutex<HashMap<u32, UsageCache>>,
    logins: Mutex<HashMap<String, Pending>>,
}

/// What a call changed: the active login is different now, so running
/// agents should start `claude` again to pick it up.
#[derive(Debug, Clone, PartialEq)]
pub struct Changed {
    pub result: Value,
    pub live_changed: bool,
}

impl<H: Http> Accounts<H> {
    pub fn new(paths: Paths, http: H) -> Self {
        Accounts {
            paths,
            http,
            lock: Mutex::new(()),
            usage: Mutex::new(HashMap::new()),
            logins: Mutex::new(HashMap::new()),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    fn index_path(&self) -> PathBuf {
        self.paths.store.join("accounts.json")
    }

    fn slot_path(&self, number: u32) -> PathBuf {
        self.paths.store.join(format!("slot-{number}.json"))
    }

    fn load(&self) -> Result<(Option<u32>, Vec<Slot>)> {
        let index = read_json(&self.index_path())?.unwrap_or_else(|| json!({}));
        let active = index
            .get("active")
            .and_then(Value::as_u64)
            .map(|n| n as u32);
        let mut slots = Vec::new();
        for n in index
            .get("accounts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            let number = n as u32;
            if let Some(data) = read_json(&self.slot_path(number))? {
                slots.push(Slot { number, data });
            }
        }
        slots.sort_by_key(|s| s.number);
        let active = active.filter(|a| slots.iter().any(|s| s.number == *a));
        Ok((active, slots))
    }

    fn save_index(&self, active: Option<u32>, slots: &[Slot]) -> Result<()> {
        let numbers: Vec<u32> = slots.iter().map(|s| s.number).collect();
        write_json(
            &self.index_path(),
            &json!({ "schemaVersion": 1, "active": active, "accounts": numbers }),
            false,
        )
    }

    fn save_slot(&self, slot: &Slot) -> Result<()> {
        write_json(&self.slot_path(slot.number), &slot.data, true)
    }

    /// Claude Code's live login: its `claudeAiOauth` and `oauthAccount`.
    fn live(&self) -> Result<(Option<Value>, Option<Value>)> {
        let oauth = read_json(&self.paths.credentials())?
            .and_then(|c| c.get("claudeAiOauth").cloned())
            .filter(|o| str_of(o, "accessToken").is_some());
        let account = read_json(&self.paths.global_config)?
            .and_then(|c| c.get("oauthAccount").cloned())
            .filter(Value::is_object);
        Ok((oauth, account))
    }

    /// Makes Claude Code use `slot`: its credentials into `.credentials.json`
    /// (other keys kept, e.g. MCP tokens) and its identity into the global
    /// config.
    fn write_live(&self, slot: &Slot) -> Result<()> {
        let path = self.paths.credentials();
        let mut creds = read_json(&path)?
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        creds["claudeAiOauth"] = slot.oauth().clone();
        write_json(&path, &creds, true)?;
        let mut config = read_json(&self.paths.global_config)?
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        config["oauthAccount"] = slot.account().clone();
        write_json(&self.paths.global_config, &config, false)
    }

    /// Brings the store in line with the live login: an account signed in
    /// by other means is adopted, the live (refreshed) tokens of the active
    /// account are kept in its slot, and the active slot is the live one.
    /// Returns `(active, slots, signed_in)`.
    fn reconcile(&self) -> Result<(Option<u32>, Vec<Slot>, bool)> {
        let (mut active, mut slots) = self.load()?;
        let (oauth, account) = self.live()?;
        let Some(oauth) = oauth else {
            return Ok((active, slots, false));
        };
        let uuid = account
            .as_ref()
            .and_then(|a| str_of(a, "accountUuid"))
            .map(str::to_string);
        let live_slot = match &uuid {
            Some(uuid) => slots.iter().position(|s| s.uuid() == Some(uuid.as_str())),
            // Credentials with no identity: they are the active slot's.
            None => active.and_then(|a| slots.iter().position(|s| s.number == a)),
        };
        let index = match live_slot {
            Some(i) => i,
            None if uuid.is_some() => {
                let number = slots.iter().map(|s| s.number).max().unwrap_or(0) + 1;
                slots.push(Slot {
                    number,
                    data: json!({ "claudeAiOauth": oauth, "oauthAccount": account }),
                });
                slots.len() - 1
            }
            None => return Ok((active, slots, true)),
        };
        let slot = &mut slots[index];
        let mut changed = slot.data["claudeAiOauth"] != oauth;
        if changed {
            slot.data["claudeAiOauth"] = oauth;
        }
        if let Some(account) = account {
            if slot.data["oauthAccount"] != account {
                slot.data["oauthAccount"] = account;
                changed = true;
            }
        }
        if changed || live_slot.is_none() {
            self.save_slot(slot)?;
        }
        let number = slot.number;
        if active != Some(number) || live_slot.is_none() {
            active = Some(number);
            self.save_index(active, &slots)?;
        }
        Ok((active, slots, true))
    }

    /// Every account with its usage, as `cswap list --json` prints it, plus
    /// `managed` and `signedIn` (a live login exists).
    pub fn list(&self) -> Result<Value> {
        let (active, mut slots, signed_in) = {
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            self.reconcile()?
        };
        let now = now_ms();
        let now_s = (now / 1000) as i64;
        let mut accounts = Vec::new();
        for slot in slots.iter_mut() {
            let is_active = active == Some(slot.number);
            let (usage, last_good, status) = self.usage_of(slot, is_active, now);
            let org = str_of(slot.account(), "organizationName");
            accounts.push(json!({
                "number": slot.number,
                "email": slot.email(),
                "organizationName": org,
                "active": is_active,
                "usageStatus": status,
                "usage": usage.map(|u| refresh_countdowns(&u, now_s)),
                "lastGoodUsage": last_good.map(|u| refresh_countdowns(&u, now_s)),
            }));
        }
        Ok(json!({
            "schemaVersion": 1,
            "managed": true,
            "signedIn": signed_in && active.is_some(),
            "activeAccountNumber": active,
            "accounts": accounts,
        }))
    }

    /// Usage of one account, from the cache or the endpoint. An inactive
    /// account's expired token is refreshed (its slot is the only copy);
    /// the active one is Claude Code's to refresh.
    fn usage_of(
        &self,
        slot: &mut Slot,
        is_active: bool,
        now: u64,
    ) -> (Option<Value>, Option<Value>, String) {
        {
            let cache = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(c) = cache.get(&slot.number) {
                if c.at.is_some_and(|at| at.elapsed() < USAGE_TTL) {
                    return (c.usage.clone(), c.last_good.clone(), c.status.clone());
                }
            }
        }
        let fetched = self.fetch_usage(slot, is_active, now);
        let mut cache = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
        let c = cache.entry(slot.number).or_default();
        c.at = Some(Instant::now());
        match fetched {
            Ok(usage) => {
                c.status = "ok".into();
                if usage.is_some() {
                    c.last_good = usage.clone();
                }
                c.usage = usage;
            }
            Err(status) => {
                c.status = status;
                c.usage = None;
            }
        }
        (c.usage.clone(), c.last_good.clone(), c.status.clone())
    }

    fn fetch_usage(
        &self,
        slot: &mut Slot,
        is_active: bool,
        now: u64,
    ) -> std::result::Result<Option<Value>, String> {
        if expired(slot.oauth(), now) {
            if is_active {
                return Err("expired".into());
            }
            self.refresh_slot(slot, now)?;
        }
        let token = str_of(slot.oauth(), "accessToken")
            .ok_or("relogin")?
            .to_string();
        let answer = self
            .http
            .request(
                HttpRequest::get(USAGE_URL)
                    .bearer(&token)
                    .header("anthropic-beta", OAUTH_BETA),
            )
            .map_err(|_| "error".to_string())?;
        match answer.status {
            200 => {
                let data: Value =
                    serde_json::from_str(&answer.body).map_err(|_| "error".to_string())?;
                Ok(normalize_usage(&data, (now / 1000) as i64))
            }
            401 if !is_active => {
                self.refresh_slot(slot, now)?;
                let token = str_of(slot.oauth(), "accessToken")
                    .ok_or("relogin")?
                    .to_string();
                let again = self
                    .http
                    .request(
                        HttpRequest::get(USAGE_URL)
                            .bearer(&token)
                            .header("anthropic-beta", OAUTH_BETA),
                    )
                    .map_err(|_| "error".to_string())?;
                if again.status != 200 {
                    return Err(format!("http-{}", again.status));
                }
                let data: Value =
                    serde_json::from_str(&again.body).map_err(|_| "error".to_string())?;
                Ok(normalize_usage(&data, (now / 1000) as i64))
            }
            401 => Err("expired".into()),
            status => Err(format!("http-{status}")),
        }
    }

    /// Refresh-token grant for a stored (inactive) account; the rotated
    /// tokens are saved in its slot.
    fn refresh_slot(&self, slot: &mut Slot, now: u64) -> std::result::Result<(), String> {
        let refresh = str_of(slot.oauth(), "refreshToken")
            .ok_or("relogin")?
            .to_string();
        let body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLIENT_ID,
        });
        let answer = self
            .http
            .request(HttpRequest::post_json(TOKEN_URL, &body))
            .map_err(|_| "error".to_string())?;
        if matches!(answer.status, 400 | 401 | 403) {
            return Err("relogin".into());
        }
        if answer.status != 200 {
            return Err(format!("http-{}", answer.status));
        }
        let token: Value = serde_json::from_str(&answer.body).map_err(|_| "error".to_string())?;
        let mut oauth = slot.oauth().clone();
        let access = str_of(&token, "access_token").ok_or("error")?;
        oauth["accessToken"] = json!(access);
        oauth["expiresAt"] = json!(
            now + token
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(3600)
                * 1000
        );
        if let Some(r) = str_of(&token, "refresh_token") {
            oauth["refreshToken"] = json!(r);
        }
        if let Some(scope) = str_of(&token, "scope") {
            oauth["scopes"] = json!(scope.split_whitespace().collect::<Vec<_>>());
        }
        slot.data["claudeAiOauth"] = oauth;
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.save_slot(slot).map_err(|_| "error".to_string())
    }

    /// Makes account `number` the machine's login.
    pub fn switch(&self, number: u32) -> Result<Changed> {
        let changed = {
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (active, slots, _) = self.reconcile()?;
            let target = slots
                .iter()
                .find(|s| s.number == number)
                .ok_or_else(|| format!("no account {number}"))?;
            if active == Some(number) {
                false
            } else {
                self.write_live(target)?;
                self.save_index(Some(number), &slots)?;
                true
            }
        };
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&number);
        Ok(Changed {
            result: self.list()?,
            live_changed: changed,
        })
    }

    /// Drops an account from the store (not the active one: switch first).
    pub fn remove(&self, number: u32) -> Result<Value> {
        {
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (active, mut slots, _) = self.reconcile()?;
            if active == Some(number) {
                return Err(
                    "this account is the machine's login now: switch to another one first".into(),
                );
            }
            let before = slots.len();
            slots.retain(|s| s.number != number);
            if slots.len() == before {
                return Err(format!("no account {number}"));
            }
            self.save_index(active, &slots)?;
            let _ = std::fs::remove_file(self.slot_path(number));
        }
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&number);
        self.list()
    }

    /// Starts a sign-in: `{login_id, url}`. The user opens the URL, signs
    /// in and gets a code to paste into [`Accounts::login_finish`].
    pub fn login_start(&self) -> Result<Value> {
        let verifier = random_token(32)?;
        let state = random_token(32)?;
        let login_id = random_token(12)?;
        let url = authorize_url(&code_challenge(&verifier), &state);
        let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
        logins.retain(|_, p| p.started.elapsed() < LOGIN_TTL);
        logins.insert(
            login_id.clone(),
            Pending {
                verifier,
                state,
                started: Instant::now(),
            },
        );
        Ok(json!({ "login_id": login_id, "url": url }))
    }

    /// Finishes a sign-in with the pasted code: exchanges it, stores the
    /// account (a known account gets its new tokens) and, when the machine
    /// has no login yet or `activate` is set, makes it the login.
    pub fn login_finish(&self, login_id: &str, pasted: &str, activate: bool) -> Result<Changed> {
        let (code, pasted_state) = parse_pasted_code(pasted)?;
        let pending = {
            let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
            let pending = logins
                .get(login_id)
                .filter(|p| p.started.elapsed() < LOGIN_TTL)
                .ok_or("this sign-in has expired: start it again")?;
            if pasted_state.as_deref().is_some_and(|s| s != pending.state) {
                return Err("this code belongs to another sign-in: use the link shown here".into());
            }
            logins.remove(login_id).expect("checked above")
        };
        let body = json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": MANUAL_REDIRECT_URL,
            "client_id": CLIENT_ID,
            "code_verifier": pending.verifier,
            "state": pending.state,
        });
        let answer = self
            .http
            .request(HttpRequest::post_json(TOKEN_URL, &body))?;
        if answer.status != 200 {
            return Err(if answer.status == 401 || answer.status == 400 {
                "the code was not accepted (it is single-use and expires quickly): start the sign-in again".to_string()
            } else {
                format!("sign-in failed: token endpoint answered {}", answer.status)
            });
        }
        let token: Value =
            serde_json::from_str(&answer.body).map_err(|e| format!("sign-in failed: {e}"))?;
        let access = str_of(&token, "access_token").ok_or("sign-in failed: no access token")?;
        let profile = self
            .http
            .request(HttpRequest::get(PROFILE_URL).bearer(access))?;
        if profile.status != 200 {
            return Err(format!(
                "signed in, but the profile could not be read ({})",
                profile.status
            ));
        }
        let profile: Value = serde_json::from_str(&profile.body).map_err(|e| e.to_string())?;
        let account = account_from_profile(&profile)?;
        let subscription = subscription_of(
            profile
                .pointer("/organization/organization_type")
                .and_then(Value::as_str),
        );
        let oauth = oauth_from_token(&token, subscription.as_deref(), now_ms())?;
        self.store_login(oauth, account, activate)
    }

    fn store_login(&self, oauth: Value, account: Value, activate: bool) -> Result<Changed> {
        let number;
        let live_changed;
        {
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (active, mut slots, signed_in) = self.reconcile()?;
            let uuid = str_of(&account, "accountUuid").map(str::to_string);
            let data = json!({ "claudeAiOauth": oauth, "oauthAccount": account });
            let index = match slots.iter().position(|s| s.uuid() == uuid.as_deref()) {
                Some(i) => {
                    slots[i].data = data;
                    i
                }
                None => {
                    let n = slots.iter().map(|s| s.number).max().unwrap_or(0) + 1;
                    slots.push(Slot { number: n, data });
                    slots.len() - 1
                }
            };
            self.save_slot(&slots[index])?;
            number = slots[index].number;
            let make_live = activate || !signed_in || active.is_none() || active == Some(number);
            if make_live {
                self.write_live(&slots[index])?;
            }
            self.save_index(if make_live { Some(number) } else { active }, &slots)?;
            live_changed = make_live;
        }
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&number);
        let mut result = self.list()?;
        result["added"] = json!(number);
        Ok(Changed {
            result,
            live_changed,
        })
    }
}

/// Folders Claude Code has worked in on this machine (the `projects` of its
/// global config) that still exist, git repositories marked: what Deck
/// offers when opening a repository on this server. `[{name, root, git}]`,
/// repositories first, then by name.
pub fn known_folders(paths: &Paths) -> Vec<Value> {
    let config = read_json(&paths.global_config).ok().flatten();
    let mut out: Vec<(bool, String, String)> = config
        .as_ref()
        .and_then(|c| c.get("projects"))
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|projects| projects.keys())
        .filter_map(|root| {
            let dir = Path::new(root);
            if !dir.is_dir() {
                return None;
            }
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.clone());
            Some((dir.join(".git").exists(), name, root.clone()))
        })
        .collect();
    out.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    out.dedup_by(|a, b| a.2 == b.2);
    out.into_iter()
        .map(|(git, name, root)| json!({ "name": name, "root": root, "git": git }))
        .collect()
}

/// `~` and `~/…` against the user's home folder.
pub fn expand_home(path: &str) -> PathBuf {
    let home = || {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    };
    match path.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| PathBuf::from(path)),
        Some(rest) if rest.starts_with(['/', '\\']) => home()
            .map(|h| h.join(&rest[1..]))
            .unwrap_or_else(|| PathBuf::from(path)),
        _ => PathBuf::from(path),
    }
}

/// What is at `path` on this machine: `{path, exists, dir}` (`path` with
/// `~` expanded), for Deck's OPEN REPOSITORY before it opens a space there.
pub fn folder_state(path: &str) -> Result<Value> {
    if path.trim().is_empty() {
        return Err("a path is needed".into());
    }
    let p = expand_home(path.trim());
    let meta = std::fs::metadata(&p).ok();
    Ok(json!({
        "path": p.to_string_lossy(),
        "exists": meta.is_some(),
        "dir": meta.is_some_and(|m| m.is_dir()),
    }))
}

/// Creates the folder (and its parents) on this machine; its state after.
pub fn make_folder(path: &str) -> Result<Value> {
    if path.trim().is_empty() {
        return Err("a path is needed".into());
    }
    let p = expand_home(path.trim());
    if !p.is_absolute() {
        return Err(format!("{} is not an absolute path", p.display()));
    }
    std::fs::create_dir_all(&p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    folder_state(&p.to_string_lossy())
}

/// The authorize URL for a pasted-code sign-in with a Claude subscription.
pub fn authorize_url(challenge: &str, state: &str) -> String {
    let params = [
        ("code", "true".to_string()),
        ("client_id", CLIENT_ID.to_string()),
        ("response_type", "code".to_string()),
        ("redirect_uri", MANUAL_REDIRECT_URL.to_string()),
        ("scope", SCOPES.join(" ")),
        ("code_challenge", challenge.to_string()),
        ("code_challenge_method", "S256".to_string()),
        ("state", state.to_string()),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect();
    format!("{AUTHORIZE_URL}?{}", query.join("&"))
}

#[cfg(test)]
mod tests;
