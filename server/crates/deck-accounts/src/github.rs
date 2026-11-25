//! `deck.github.*`: this machine signs in to GitHub, for git over HTTPS to
//! private repositories (a clone that was refused, [`crate::clone`]).
//!
//! GitHub's OAuth device flow, run by the server: [`GitHub::login_start`]
//! asks GitHub for a code, Deck shows it with the link, the user approves
//! in any browser, and a background thread here polls until GitHub hands
//! out the token. The token is kept only on this machine, in the server's
//! config dir (`github/token.json`, owner-only on Unix); it is given to a
//! git process in its environment when one needs it, never written into a
//! repository, never logged and never sent to Deck.
//!
//! Kept apart from Claude's accounts so the per-machine connections that
//! come later (Settings) can reuse it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::http::{Http, HttpRequest};

/// The OAuth App Deck signs in with (device flow enabled, no secret needed).
///
/// TODO: create the GitHub OAuth App for Netrunner Deck (Settings →
/// Developer settings → OAuth Apps → New, "Enable Device Flow" checked) and
/// put its client id here. Until then sign-in reports that it is not set
/// up; `DECK_GITHUB_CLIENT_ID` in the server's environment overrides it.
pub const CLIENT_ID: &str = "";
pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const USER_URL: &str = "https://api.github.com/user";
/// Private repositories over HTTPS.
pub const SCOPE: &str = "repo";

/// The client id in use, if any.
pub fn client_id() -> Option<String> {
    std::env::var("DECK_GITHUB_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| (!CLIENT_ID.is_empty()).then(|| CLIENT_ID.to_string()))
}

fn form(url: &str, fields: &[(&str, &str)]) -> HttpRequest {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", crate::percent_encode(v)))
        .collect();
    HttpRequest {
        method: "POST",
        url: url.into(),
        headers: vec![
            (
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
            ("Accept".into(), "application/json".into()),
        ],
        body: Some(body.join("&")),
    }
}

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

/// One answer of the token endpoint while the user has not decided yet.
#[derive(Debug, PartialEq)]
pub enum Poll {
    Token(String),
    Pending,
    /// GitHub asks to wait longer between polls.
    SlowDown,
    Failed(String),
}

/// Asks once whether the user approved `device_code`.
pub fn poll_once(http: &dyn Http, client_id: &str, device_code: &str) -> Poll {
    let request = form(
        TOKEN_URL,
        &[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    );
    let answer = match http.request(request) {
        Ok(a) => a,
        // The network may come back: keep asking until the code expires.
        Err(_) => return Poll::Pending,
    };
    let v = parse(&answer.body);
    if let Some(token) = v
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        return Poll::Token(token.to_string());
    }
    match v.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown,
        Some("expired_token") => Poll::Failed("the code expired — start the sign-in again".into()),
        Some("access_denied") => Poll::Failed("the sign-in was declined on GitHub".into()),
        Some(other) => Poll::Failed(
            v.get("error_description")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| other.to_string()),
        ),
        None => Poll::Failed(format!("GitHub answered {} without a token", answer.status)),
    }
}

/// The GitHub login of a token (`None` when GitHub does not say).
pub fn user_of(http: &dyn Http, token: &str) -> Option<String> {
    let request = HttpRequest::get(USER_URL)
        .bearer(token)
        .header("Accept", "application/vnd.github+json");
    let answer = http.request(request).ok()?;
    (answer.status == 200)
        .then(|| {
            parse(&answer.body)
                .get("login")?
                .as_str()
                .map(str::to_string)
        })
        .flatten()
}

#[derive(Debug, Clone)]
struct Login {
    state: &'static str,
    user: Option<String>,
    message: Option<String>,
    started: Instant,
}

/// The machine's GitHub sign-in: its stored token and the sign-ins in flight.
pub struct GitHub<H: Http + 'static> {
    store: PathBuf,
    http: Arc<H>,
    logins: Arc<Mutex<HashMap<String, Login>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<H: Http + 'static> GitHub<H> {
    /// `store` is the folder of `token.json` (`<server config dir>/github`).
    pub fn new(store: PathBuf, http: H) -> Self {
        GitHub {
            store,
            http: Arc::new(http),
            logins: Arc::default(),
        }
    }

    fn token_file(&self) -> PathBuf {
        self.store.join("token.json")
    }

    /// The stored token, for a git process that needs it.
    pub fn token(&self) -> Option<String> {
        let v = crate::read_json(&self.token_file()).ok().flatten()?;
        v.get("access_token")?
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    }

    /// `deck.github.status` → `{configured, signed_in, user}` (no token).
    pub fn status(&self) -> Value {
        let v = crate::read_json(&self.token_file()).ok().flatten();
        let user = v
            .as_ref()
            .and_then(|v| v.get("user"))
            .cloned()
            .unwrap_or(Value::Null);
        json!({
            "configured": client_id().is_some(),
            "signed_in": self.token().is_some(),
            "user": user,
        })
    }

    /// `deck.github.logout`: forgets the token.
    pub fn logout(&self) -> crate::Result<Value> {
        match std::fs::remove_file(self.token_file()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove the GitHub token: {e}")),
        }
        Ok(self.status())
    }

    fn save(store: &std::path::Path, token: &str, user: Option<&str>) -> crate::Result<()> {
        crate::write_json(
            &store.join("token.json"),
            &json!({
                "access_token": token,
                "scope": SCOPE,
                "user": user,
                "saved_ms": crate::now_ms(),
            }),
            true,
        )
    }

    /// `deck.github.login.start` → `{login_id, user_code, verification_uri,
    /// expires_in}`; the server polls GitHub until the user decides.
    pub fn login_start(&self) -> crate::Result<Value> {
        self.login_start_with(std::thread::sleep)
    }

    /// [`GitHub::login_start`] with its own sleep (tests poll at once).
    pub fn login_start_with(
        &self,
        sleep: impl Fn(Duration) + Send + 'static,
    ) -> crate::Result<Value> {
        let client = client_id()
            .ok_or("GitHub sign-in is not set up in this deck-server build (no OAuth client id)")?;
        let answer = self.http.request(form(
            DEVICE_CODE_URL,
            &[("client_id", &client), ("scope", SCOPE)],
        ))?;
        let v = parse(&answer.body);
        let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(device_code), Some(user_code), Some(uri)) = (
            field("device_code"),
            field("user_code"),
            field("verification_uri"),
        ) else {
            return Err(field("error_description")
                .or_else(|| field("error"))
                .map(|e| format!("GitHub refused the sign-in: {e}"))
                .unwrap_or_else(|| format!("GitHub answered {} without a code", answer.status)));
        };
        let expires = v.get("expires_in").and_then(Value::as_u64).unwrap_or(900);
        let mut interval = v
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .max(1);
        let id = crate::random_id()?;
        lock(&self.logins).retain(|_, l| l.started.elapsed() < Duration::from_secs(3600));
        lock(&self.logins).insert(
            id.clone(),
            Login {
                state: "pending",
                user: None,
                message: None,
                started: Instant::now(),
            },
        );
        let (http, logins, store, login) = (
            self.http.clone(),
            self.logins.clone(),
            self.store.clone(),
            id.clone(),
        );
        std::thread::Builder::new()
            .name("deck-github-login".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(expires);
                let set = |state: &'static str, user: Option<String>, message: Option<String>| {
                    if let Some(l) = lock(&logins).get_mut(&login) {
                        l.state = state;
                        l.user = user;
                        l.message = message;
                    }
                };
                loop {
                    sleep(Duration::from_secs(interval));
                    if Instant::now() > deadline {
                        set(
                            "failed",
                            None,
                            Some("the code expired — start the sign-in again".into()),
                        );
                        return;
                    }
                    match poll_once(&*http, &client, &device_code) {
                        Poll::Pending => {}
                        Poll::SlowDown => interval += 5,
                        Poll::Failed(m) => return set("failed", None, Some(m)),
                        Poll::Token(token) => {
                            let user = user_of(&*http, &token);
                            return match Self::save(&store, &token, user.as_deref()) {
                                Ok(()) => set("done", user, None),
                                Err(e) => set("failed", None, Some(e)),
                            };
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot start the sign-in: {e}"))?;
        Ok(json!({
            "login_id": id,
            "user_code": user_code,
            "verification_uri": uri,
            "expires_in": expires,
        }))
    }

    /// `deck.github.login.status {login_id}` → `{state: pending|done|failed,
    /// user, message}`.
    pub fn login_status(&self, login_id: &str) -> crate::Result<Value> {
        let logins = lock(&self.logins);
        let l = logins
            .get(login_id)
            .ok_or("this sign-in is not known on the machine any more — start it again")?;
        Ok(json!({ "state": l.state, "user": l.user, "message": l.message }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;

    /// Answers by URL in order (the last answer repeats); records requests.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<HashMap<String, Vec<(u16, Value)>>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Fake {
        fn answer(&self, url: &str, status: u16, body: Value) -> &Self {
            lock(&self.answers)
                .entry(url.into())
                .or_default()
                .push((status, body));
            self
        }
    }

    impl Http for Fake {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse, String> {
            lock(&self.seen).push(request.clone());
            let mut answers = lock(&self.answers);
            let list = answers.get_mut(&request.url).ok_or("offline")?;
            let (status, body) = if list.len() > 1 {
                list.remove(0)
            } else {
                list[0].clone()
            };
            Ok(HttpResponse {
                status,
                body: body.to_string(),
            })
        }
    }

    impl Http for Arc<Fake> {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse, String> {
            (**self).request(request)
        }
    }

    fn store() -> PathBuf {
        std::env::temp_dir().join(format!(
            "deck-github-test-{}-{}-{}",
            std::process::id(),
            crate::now_ms(),
            crate::random_id().unwrap()
        ))
    }

    #[test]
    fn polling_reads_githubs_answers() {
        let f = Fake::default();
        f.answer(TOKEN_URL, 200, json!({"error": "authorization_pending"}))
            .answer(TOKEN_URL, 200, json!({"error": "slow_down"}))
            .answer(TOKEN_URL, 200, json!({"error": "access_denied"}))
            .answer(
                TOKEN_URL,
                200,
                json!({"access_token": "gho_x", "token_type": "bearer"}),
            );
        assert_eq!(poll_once(&f, "cid", "dev"), Poll::Pending);
        assert_eq!(poll_once(&f, "cid", "dev"), Poll::SlowDown);
        assert!(matches!(poll_once(&f, "cid", "dev"), Poll::Failed(m) if m.contains("declined")));
        assert_eq!(poll_once(&f, "cid", "dev"), Poll::Token("gho_x".into()));
        let seen = lock(&f.seen);
        let body = seen[0].body.as_deref().unwrap();
        assert!(body.contains("client_id=cid") && body.contains("device_code=dev"));
        assert!(body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
        assert!(seen[0]
            .headers
            .iter()
            .any(|(k, v)| k == "Accept" && v == "application/json"));
    }

    #[test]
    fn a_device_sign_in_stores_the_token_on_the_machine_only() {
        std::env::set_var("DECK_GITHUB_CLIENT_ID", "test-client");
        let fake = Arc::new(Fake::default());
        fake.answer(DEVICE_CODE_URL, 200, json!({
            "device_code": "dev", "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5
        }));
        fake.answer(TOKEN_URL, 200, json!({"error": "authorization_pending"}))
            .answer(TOKEN_URL, 200, json!({"access_token": "gho_secret"}));
        fake.answer(USER_URL, 200, json!({"login": "octo"}));
        let dir = store();
        let gh = GitHub::new(dir.clone(), fake.clone());
        assert_eq!(gh.status()["signed_in"], json!(false));
        let started = gh.login_start_with(|_| {}).unwrap();
        assert_eq!(started["user_code"], json!("ABCD-1234"));
        assert_eq!(
            started["verification_uri"],
            json!("https://github.com/login/device")
        );
        assert!(
            started.get("device_code").is_none(),
            "the device code stays on the machine"
        );
        let id = started["login_id"].as_str().unwrap().to_string();
        let mut state = Value::Null;
        for _ in 0..200 {
            state = gh.login_status(&id).unwrap();
            if state["state"] != json!("pending") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(state["state"], json!("done"), "{state}");
        assert_eq!(state["user"], json!("octo"));
        assert!(
            state.to_string().find("gho_secret").is_none(),
            "no token in a reply"
        );
        assert_eq!(gh.token().as_deref(), Some("gho_secret"));
        let status = gh.status();
        assert_eq!(status["signed_in"], json!(true));
        assert_eq!(status["user"], json!("octo"));
        assert!(!status.to_string().contains("gho_secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("token.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        gh.logout().unwrap();
        assert_eq!(gh.token(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_sign_in_says_so() {
        let gh = GitHub::new(store(), Fake::default());
        assert!(gh
            .login_status("nope")
            .unwrap_err()
            .contains("start it again"));
    }
}
