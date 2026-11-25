//! HTTPS for the accounts module, through the system's `curl`.
//!
//! deck-server is a static binary with no TLS stack of its own; `curl` is
//! on every Linux machine it runs on and in Windows since 10 1803
//! (`System32\curl.exe`). Nothing secret goes on the command line: the
//! URL, headers and body are written to curl's stdin as its config
//! (`-K -`), so tokens never show in the process list.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn get(url: &str) -> Self {
        HttpRequest {
            method: "GET",
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post_json(url: &str, body: &Value) -> Self {
        HttpRequest {
            method: "POST",
            url: url.into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(body.to_string()),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", &format!("Bearer {token}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait Http: Send + Sync {
    /// `Err` only when no answer came (network, curl missing); an HTTP
    /// error status is an `Ok` answer.
    fn request(&self, request: HttpRequest) -> Result<HttpResponse, String>;
}

/// [`Http`] over the system's `curl`.
pub struct Curl {
    program: PathBuf,
}

impl Default for Curl {
    fn default() -> Self {
        Curl {
            program: find_curl(),
        }
    }
}

pub(crate) fn find_curl() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(root).join("System32").join("curl.exe");
            if p.is_file() {
                return p;
            }
        }
        PathBuf::from("curl.exe")
    }
    #[cfg(not(windows))]
    {
        for p in ["/usr/bin/curl", "/bin/curl", "/usr/local/bin/curl"] {
            if std::path::Path::new(p).is_file() {
                return PathBuf::from(p);
            }
        }
        PathBuf::from("curl")
    }
}

/// A string in curl's config syntax (double quotes, backslash escapes).
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `-K -` config for one request; the status code comes last on its
/// own line (`write-out`).
pub fn curl_config(request: &HttpRequest) -> String {
    let mut cfg = String::new();
    cfg.push_str(&format!("url = {}\n", quoted(&request.url)));
    cfg.push_str(&format!("request = {}\n", quoted(request.method)));
    cfg.push_str(&format!(
        "user-agent = {}\n",
        quoted(concat!("deck-server/", env!("CARGO_PKG_VERSION")))
    ));
    for (name, value) in &request.headers {
        cfg.push_str(&format!(
            "header = {}\n",
            quoted(&format!("{name}: {value}"))
        ));
    }
    if let Some(body) = &request.body {
        cfg.push_str(&format!("data-binary = {}\n", quoted(body)));
    }
    cfg.push_str("silent\nshow-error\nmax-time = 20\n");
    cfg.push_str("write-out = \"\\n%{http_code}\"\n");
    cfg
}

/// Splits curl's output into the body and the status line `write-out` added.
pub fn parse_curl_output(out: &str) -> Option<HttpResponse> {
    let (body, code) = out.rsplit_once('\n')?;
    let status: u16 = code.trim().parse().ok()?;
    (status != 0).then(|| HttpResponse {
        status,
        body: body.to_string(),
    })
}

impl Http for Curl {
    fn request(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let mut command = Command::new(&self.program);
        command
            .args(["-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.program.display()))?;
        let config = curl_config(&request);
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|e| format!("curl: {e}"))?;
        }
        // stdin is closed here; stdout and stderr are read together.
        let output = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = String::from_utf8_lossy(&output.stdout);
        parse_curl_output(&text).ok_or_else(|| {
            let why = stderr.trim();
            if why.is_empty() {
                format!("no answer from {}", request.url)
            } else {
                format!("no answer from {}: {why}", request.url)
            }
        })
    }
}
