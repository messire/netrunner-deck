use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Answers by URL; records every request.
#[derive(Default)]
struct Fake {
    answers: Mutex<HashMap<String, Vec<HttpResponse>>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl Fake {
    fn answer(&self, url: &str, status: u16, body: Value) {
        self.answers
            .lock()
            .unwrap()
            .entry(url.into())
            .or_default()
            .push(HttpResponse {
                status,
                body: body.to_string(),
            });
    }
    fn seen(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Http for &Fake {
    fn request(&self, request: HttpRequest) -> std::result::Result<HttpResponse, String> {
        self.seen.lock().unwrap().push(request.clone());
        let mut answers = self.answers.lock().unwrap();
        let list = answers.get_mut(&request.url).ok_or("offline")?;
        if list.len() > 1 {
            Ok(list.remove(0))
        } else {
            list.first().cloned().ok_or_else(|| "offline".to_string())
        }
    }
}

fn temp_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "deck-accounts-test-{}-{}-{}",
        std::process::id(),
        now_ms(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn paths() -> Paths {
    let root = temp_dir();
    Paths {
        store: root.join("store"),
        claude_home: root.join(".claude"),
        global_config: root.join(".claude.json"),
    }
}

fn live_login(p: &Paths, uuid: &str, email: &str, access: &str) {
    write_json(
        &p.credentials(),
        &json!({
            "claudeAiOauth": { "accessToken": access, "refreshToken": format!("r-{access}"), "expiresAt": now_ms() + 3_600_000 },
            "mcpOAuth": { "keep": "me" },
        }),
        true,
    )
    .unwrap();
    let mut config = read_json(&p.global_config)
        .unwrap()
        .unwrap_or_else(|| json!({ "theme": "dark" }));
    config["oauthAccount"] =
        json!({ "accountUuid": uuid, "emailAddress": email, "organizationName": "Org" });
    write_json(&p.global_config, &config, false).unwrap();
}

fn usage_body(five: f64) -> Value {
    json!({
        "five_hour": { "utilization": five, "resets_at": "2099-01-01T00:00:00+00:00" },
        "seven_day": { "utilization": 25.0, "resets_at": null },
        "limits": [{ "scope": { "model": { "display_name": "Fable" } }, "percent": 3.0 }],
    })
}

#[test]
fn rfc3339_and_countdown() {
    assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339("2026-12-01T00:59:59.749287+00:00"),
        Some(1_796_086_799)
    );
    assert_eq!(
        parse_rfc3339("2026-12-01T02:59:59+02:00"),
        Some(1_796_086_799)
    );
    assert_eq!(parse_rfc3339("nope"), None);
    let at = "1970-01-07T04:30:00Z";
    assert_eq!(countdown(at, 0).as_deref(), Some("6d 4h"));
    assert_eq!(countdown(at, 6 * 86400).as_deref(), Some("4h 30m"));
    assert_eq!(
        countdown(at, 6 * 86400 + 4 * 3600 + 20 * 60).as_deref(),
        Some("10m")
    );
    assert_eq!(countdown(at, i64::MAX / 2).as_deref(), Some("0m"));
}

#[test]
fn usage_has_cswap_shape() {
    let u = normalize_usage(&usage_body(12.5), 0).unwrap();
    assert_eq!(u["fiveHour"]["pct"], 12.5);
    assert!(u["fiveHour"]["countdown"].is_string());
    assert_eq!(u["sevenDay"]["pct"], 25.0);
    assert!(u["sevenDay"].get("countdown").is_none());
    assert_eq!(u["scoped"][0]["name"], "Fable");
    assert_eq!(normalize_usage(&json!({}), 0), None);
}

#[test]
fn pasted_codes() {
    assert_eq!(
        parse_pasted_code(" abc#st ").unwrap(),
        ("abc".into(), Some("st".into()))
    );
    assert_eq!(parse_pasted_code("abc").unwrap(), ("abc".into(), None));
    assert_eq!(
        parse_pasted_code("https://platform.claude.com/oauth/code/callback?code=a%2Bb&state=s1")
            .unwrap(),
        ("a+b".into(), Some("s1".into()))
    );
    assert!(parse_pasted_code("  ").is_err());
    assert!(parse_pasted_code("#st").is_err());
}

#[test]
fn authorize_url_is_claudes_manual_flow() {
    let url = authorize_url(&code_challenge("verifier"), "the-state");
    assert!(url.starts_with("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-"));
    assert!(
        url.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback")
    );
    assert!(url.contains("scope=org%3Acreate_api_key%20user%3Aprofile%20user%3Ainference"));
    assert!(url.contains("code_challenge_method=S256&state=the-state"));
    // RFC 7636 appendix B.
    assert_eq!(
        code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn curl_config_escapes_and_parses() {
    let req = HttpRequest::post_json("https://x.example/t", &json!({"a":"b\"c\\d"})).bearer("tok");
    let cfg = http::curl_config(&req);
    assert!(cfg.contains("url = \"https://x.example/t\"\n"));
    assert!(cfg.contains("header = \"Authorization: Bearer tok\"\n"));
    assert!(cfg.contains(r#"data-binary = "{\"a\":\"b\\\"c\\\\d\"}""#));
    assert_eq!(
        http::parse_curl_output("{\"x\":1}\n200"),
        Some(HttpResponse {
            status: 200,
            body: "{\"x\":1}".into()
        })
    );
    assert_eq!(http::parse_curl_output("\n000"), None);
}

#[test]
fn a_live_login_is_adopted_with_usage() {
    let p = paths();
    let fake = Fake::default();
    fake.answer(USAGE_URL, 200, usage_body(40.0));
    let accounts = Accounts::new(p.clone(), &fake);
    // Nothing yet.
    let empty = accounts.list().unwrap();
    assert_eq!(empty["signedIn"], false);
    assert_eq!(empty["accounts"], json!([]));
    live_login(&p, "u1", "one@example.com", "a1");
    let list = accounts.list().unwrap();
    assert_eq!(list["signedIn"], true);
    assert_eq!(list["activeAccountNumber"], 1);
    let a = &list["accounts"][0];
    assert_eq!(
        (a["email"].as_str(), a["active"].as_bool()),
        (Some("one@example.com"), Some(true))
    );
    assert_eq!(a["usageStatus"], "ok");
    assert_eq!(a["usage"]["fiveHour"]["pct"], 40.0);
    let seen = fake.seen();
    assert!(seen[0]
        .headers
        .contains(&("Authorization".into(), "Bearer a1".into())));
    // Cached: no second request.
    accounts.list().unwrap();
    assert_eq!(fake.seen().len(), 1);
}

#[test]
fn switching_saves_the_old_tokens_and_writes_the_new_login() {
    let p = paths();
    let fake = Fake::default();
    fake.answer(USAGE_URL, 200, usage_body(1.0));
    let accounts = Accounts::new(p.clone(), &fake);
    live_login(&p, "u1", "one@example.com", "a1");
    accounts.list().unwrap();
    live_login(&p, "u2", "two@example.com", "a2");
    let list = accounts.list().unwrap();
    assert_eq!(list["activeAccountNumber"], 2);
    // Claude refreshed account 2's token in place.
    live_login(&p, "u2", "two@example.com", "a2-rotated");

    let changed = accounts.switch(1).unwrap();
    assert!(changed.live_changed);
    assert_eq!(changed.result["activeAccountNumber"], 1);
    let creds = read_json(&p.credentials()).unwrap().unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "a1");
    assert_eq!(creds["mcpOAuth"]["keep"], "me", "other credentials stay");
    let config = read_json(&p.global_config).unwrap().unwrap();
    assert_eq!(config["oauthAccount"]["accountUuid"], "u1");
    assert_eq!(config["theme"], "dark", "the rest of the config stays");
    let slot2 = read_json(&accounts.slot_path(2)).unwrap().unwrap();
    assert_eq!(slot2["claudeAiOauth"]["accessToken"], "a2-rotated");

    assert!(!accounts.switch(1).unwrap().live_changed);
    assert!(accounts.switch(9).is_err());
    assert!(accounts.remove(1).is_err(), "the active account stays");
    let after = accounts.remove(2).unwrap();
    assert_eq!(after["accounts"].as_array().unwrap().len(), 1);
}

#[test]
fn an_inactive_expired_token_is_refreshed_in_its_slot() {
    let p = paths();
    let fake = Fake::default();
    fake.answer(USAGE_URL, 200, usage_body(5.0));
    fake.answer(
        TOKEN_URL,
        200,
        json!({ "access_token": "fresh", "refresh_token": "r-fresh", "expires_in": 3600 }),
    );
    let accounts = Accounts::new(p.clone(), &fake);
    live_login(&p, "u1", "one@example.com", "a1");
    accounts.list().unwrap();
    live_login(&p, "u2", "two@example.com", "a2");
    accounts.list().unwrap();
    // Account 1 is inactive now; make its stored token expired.
    let mut slot1 = read_json(&accounts.slot_path(1)).unwrap().unwrap();
    slot1["claudeAiOauth"]["expiresAt"] = json!(1);
    write_json(&accounts.slot_path(1), &slot1, true).unwrap();
    accounts.usage.lock().unwrap().clear();
    let list = accounts.list().unwrap();
    assert_eq!(list["accounts"][0]["usageStatus"], "ok");
    let slot1 = read_json(&accounts.slot_path(1)).unwrap().unwrap();
    assert_eq!(slot1["claudeAiOauth"]["accessToken"], "fresh");
    assert_eq!(slot1["claudeAiOauth"]["refreshToken"], "r-fresh");
    let refresh = fake
        .seen()
        .into_iter()
        .find(|r| r.url == TOKEN_URL)
        .unwrap();
    assert!(refresh
        .body
        .unwrap()
        .contains("\"grant_type\":\"refresh_token\""));
}

#[test]
fn sign_in_exchanges_the_pasted_code_and_becomes_the_login() {
    let p = paths();
    let fake = Fake::default();
    fake.answer(
        TOKEN_URL,
        200,
        json!({ "access_token": "acc", "refresh_token": "ref", "expires_in": 28800, "scope": "user:inference user:profile" }),
    );
    fake.answer(
        PROFILE_URL,
        200,
        json!({ "account": { "uuid": "u9", "email": "nine@example.com" }, "organization": { "uuid": "o9", "name": "Nine", "organization_type": "claude_max" } }),
    );
    fake.answer(USAGE_URL, 200, usage_body(0.0));
    let accounts = Accounts::new(p.clone(), &fake);
    let start = accounts.login_start().unwrap();
    let id = start["login_id"].as_str().unwrap().to_string();
    let url = start["url"].as_str().unwrap();
    let state = url.split("state=").nth(1).unwrap().to_string();
    assert!(accounts
        .login_finish(&id, "code#other-state", false)
        .is_err());
    let done = accounts
        .login_finish(&id, &format!("the-code#{state}"), false)
        .unwrap();
    assert!(done.live_changed, "the first account becomes the login");
    assert_eq!(done.result["added"], 1);
    assert_eq!(done.result["signedIn"], true);
    let creds = read_json(&p.credentials()).unwrap().unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "acc");
    assert_eq!(creds["claudeAiOauth"]["subscriptionType"], "max");
    assert_eq!(
        creds["claudeAiOauth"]["scopes"],
        json!(["user:inference", "user:profile"])
    );
    let config = read_json(&p.global_config).unwrap().unwrap();
    assert_eq!(config["oauthAccount"]["emailAddress"], "nine@example.com");
    assert_eq!(config["oauthAccount"]["organizationName"], "Nine");
    let exchange = fake
        .seen()
        .into_iter()
        .find(|r| r.url == TOKEN_URL)
        .unwrap()
        .body
        .unwrap();
    let exchange: Value = serde_json::from_str(&exchange).unwrap();
    assert_eq!(exchange["grant_type"], "authorization_code");
    assert_eq!(exchange["code"], "the-code");
    assert_eq!(exchange["redirect_uri"], MANUAL_REDIRECT_URL);
    assert_eq!(exchange["state"], state.as_str());
    assert!(exchange["code_verifier"].as_str().unwrap().len() >= 43);
    // A sign-in is used once.
    assert!(accounts
        .login_finish(&id, &format!("the-code#{state}"), false)
        .is_err());
}

#[test]
fn a_second_sign_in_is_added_without_taking_over() {
    let p = paths();
    let fake = Fake::default();
    fake.answer(
        TOKEN_URL,
        200,
        json!({ "access_token": "acc2", "refresh_token": "ref2" }),
    );
    fake.answer(
        PROFILE_URL,
        200,
        json!({ "account": { "uuid": "u2", "email": "two@example.com" } }),
    );
    fake.answer(USAGE_URL, 200, usage_body(0.0));
    let accounts = Accounts::new(p.clone(), &fake);
    live_login(&p, "u1", "one@example.com", "a1");
    accounts.list().unwrap();
    let start = accounts.login_start().unwrap();
    let done = accounts
        .login_finish(start["login_id"].as_str().unwrap(), "just-a-code", false)
        .unwrap();
    assert!(!done.live_changed);
    assert_eq!(done.result["activeAccountNumber"], 1);
    assert_eq!(done.result["accounts"].as_array().unwrap().len(), 2);
    let creds = read_json(&p.credentials()).unwrap().unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "a1");
}

#[test]
fn known_folders_are_claudes_projects_that_exist() {
    let p = paths();
    let root = p.store.parent().unwrap().to_path_buf();
    let repo = root.join("work").join("repo");
    let plain = root.join("work").join("notes");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    let gone = root.join("gone");
    let mut projects = serde_json::Map::new();
    for d in [&plain, &repo, &gone] {
        projects.insert(d.to_string_lossy().into_owned(), json!({}));
    }
    write_json(&p.global_config, &json!({ "projects": projects }), false).unwrap();
    let list = known_folders(&p);
    let names: Vec<&str> = list.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["repo", "notes"]);
    assert_eq!(list[0]["git"], true);
    assert_eq!(list[1]["git"], false);
    assert!(known_folders(&paths()).is_empty());
}
