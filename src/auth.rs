//! Sign in with GitHub for `flow serve`. Off unless it is configured, so the local tool stays a local tool.
//!
//! What it keeps is the person's public profile, in memory. GitHub's token is used once to read that
//! profile and dropped: this asks for no scopes and never acts as the person on GitHub.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const SESSION: &str = "flow_session";
pub const STATE: &str = "flow_oauth";
const SESSION_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
const SIGN_IN_WITHIN: Duration = Duration::from_secs(600);
/// GitHub's own addresses, unless a test stands in for it.
const WEB: &str = "https://github.com";
const API: &str = "https://api.github.com";

struct Config {
    id: String,
    secret: String,
    allowed: Vec<String>,
    web: String,
    api: String,
    back: String,
}

#[derive(Clone)]
struct User {
    login: String,
    name: String,
    avatar: String,
    until: Instant,
}

#[derive(Default)]
pub struct Auth {
    config: Option<Config>,
    sessions: Mutex<HashMap<String, User>>,
    pending: Mutex<HashMap<String, Instant>>,
}

/// Where a sign-in attempt ends up. The page says what happened.
pub struct Done {
    pub cookie: Option<String>,
    pub to: &'static str,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn random() -> Result<String> {
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).context("could not read the system's random source")?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// One value in a curl config, which is how the secret reaches curl without appearing in `ps`.
fn quoted(s: &str) -> Result<String> {
    if s.contains(['\n', '\r']) {
        bail!("a line break in a value sent to GitHub");
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn curl(config: &str) -> Result<Value> {
    let mut c = Command::new("curl").args(["--silent", "--show-error", "--max-time", "15", "--config", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("curl is not installed")?;
    c.stdin.take().context("no stdin")?.write_all(config.as_bytes())?;
    let o = c.wait_with_output()?;
    if !o.status.success() {
        bail!("{}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}

impl Auth {
    /// Half a configuration must stop the server: it would otherwise look locked and be open.
    pub fn from_env(port: u16) -> Result<Auth> {
        let (id, secret) = (env("FLOW_GITHUB_CLIENT_ID"), env("FLOW_GITHUB_CLIENT_SECRET"));
        let (id, secret) = match (id, secret) {
            (None, None) => return Ok(Auth::default()),
            (Some(i), Some(s)) => (i, s),
            _ => bail!("set both FLOW_GITHUB_CLIENT_ID and FLOW_GITHUB_CLIENT_SECRET, or neither"),
        };
        let allowed: Vec<String> = env("FLOW_ALLOWED_USERS").unwrap_or_default().split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        if allowed.is_empty() {
            bail!("set FLOW_ALLOWED_USERS to the GitHub usernames who may sign in. Anyone with a GitHub account could otherwise read this code");
        }
        let web = env("FLOW_GITHUB_URL").unwrap_or_else(|| WEB.into());
        let api = env("FLOW_GITHUB_URL").unwrap_or_else(|| API.into());
        let back = format!("http://localhost:{port}/auth/github/callback");
        Ok(Auth { config: Some(Config { id, secret, allowed, web, api, back }), ..Auth::default() })
    }

    pub fn required(&self) -> bool {
        self.config.is_some()
    }

    fn user(&self, cookie: Option<&str>) -> Option<User> {
        let mut all = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let token = cookie?;
        match all.get(token) {
            Some(u) if u.until > Instant::now() => Some(u.clone()),
            Some(_) => {
                all.remove(token);
                None
            }
            None => None,
        }
    }

    pub fn allows(&self, cookie: Option<&str>) -> bool {
        !self.required() || self.user(cookie).is_some()
    }

    pub fn me(&self, cookie: Option<&str>) -> Value {
        let user = self.user(cookie).map(|u| json!({ "login": u.login, "name": u.name, "avatar": u.avatar }));
        json!({ "required": self.required(), "user": user })
    }

    /// The address to send the person to, and the state that must come back with them.
    pub fn start(&self) -> Result<(String, String)> {
        let c = self.config.as_ref().context("sign-in is not set up")?;
        let state = random()?;
        let mut waiting = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        waiting.retain(|_, at| at.elapsed() < SIGN_IN_WITHIN);
        waiting.insert(state.clone(), Instant::now());
        Ok((format!("{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={state}", c.web, encode(&c.id), encode(&c.back)), state))
    }

    /// A state works once, and only in the browser that started: that is what stops a forged sign-in.
    fn expected(&self, state: &str, cookie: Option<&str>) -> bool {
        let at = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(state);
        cookie == Some(state) && at.is_some_and(|t| t.elapsed() < SIGN_IN_WITHIN)
    }

    fn profile(&self, c: &Config, code: &str) -> Result<Value> {
        if code.is_empty() || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            bail!("GitHub sent an unexpected code");
        }
        let ask = format!(
            "url = {}\nheader = \"Accept: application/json\"\nuser-agent = \"flow-control\"\ndata-urlencode = {}\ndata-urlencode = {}\ndata-urlencode = {}\ndata-urlencode = {}\n",
            quoted(&format!("{}/login/oauth/access_token", c.web))?,
            quoted(&format!("client_id={}", c.id))?,
            quoted(&format!("client_secret={}", c.secret))?,
            quoted(&format!("code={code}"))?,
            quoted(&format!("redirect_uri={}", c.back))?
        );
        let got = curl(&ask)?;
        let token = got["access_token"].as_str().with_context(|| format!("GitHub refused the sign-in: {}", got["error"].as_str().unwrap_or("no reason given")))?;
        let ask = format!(
            "url = {}\nheader = \"Accept: application/vnd.github+json\"\nheader = {}\nuser-agent = \"flow-control\"\n",
            quoted(&format!("{}/user", c.api))?,
            quoted(&format!("Authorization: Bearer {token}"))?
        );
        curl(&ask)
    }

    pub fn finish(&self, q: &HashMap<String, String>, cookie: Option<&str>) -> Done {
        let no = |to| Done { cookie: None, to };
        let Some(c) = self.config.as_ref() else { return no("/") };
        if !self.expected(q.get("state").map(String::as_str).unwrap_or(""), cookie) {
            return no("/?signin=failed");
        }
        if q.contains_key("error") {
            return no("/?signin=cancelled");
        }
        let profile = match self.profile(c, q.get("code").map(String::as_str).unwrap_or("")) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("flow serve: sign-in: {e}");
                return no("/?signin=failed");
            }
        };
        let Some(login) = profile["login"].as_str().map(str::to_string) else { return no("/?signin=failed") };
        if !c.allowed.contains(&login.to_lowercase()) {
            return no("/?signin=denied");
        }
        let Ok(token) = random() else { return no("/?signin=failed") };
        let name = profile["name"].as_str().unwrap_or(&login).to_string();
        let avatar = profile["avatar_url"].as_str().unwrap_or("").to_string();
        let mut all = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|_, u| u.until > Instant::now());
        all.insert(token.clone(), User { login, name, avatar, until: Instant::now() + SESSION_FOR });
        Done { cookie: Some(format!("{SESSION}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}", SESSION_FOR.as_secs())), to: "/" }
    }

    pub fn end(&self, cookie: Option<&str>) {
        if let Some(t) = cookie {
            self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(t);
        }
    }
}

/// One cookie out of the header a browser sends.
pub fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_cannot_break_out_of_its_line_in_the_curl_config() {
        assert_eq!(quoted("a\"b\\c").unwrap(), r#""a\"b\\c""#);
        assert!(quoted("a\nurl = \"http://elsewhere\"").is_err());
    }

    #[test]
    fn a_cookie_is_found_by_its_whole_name() {
        assert_eq!(cookie("a=1; flow_session=xyz; b=2", SESSION), Some("xyz"));
        assert_eq!(cookie("not_flow_session=xyz", SESSION), None);
    }
}
