//! `flow serve` shows source code, so what it refuses matters as much as what it shows.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new(args[0]).args(&args[1..]).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{args:?}");
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Server(Child, u16);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn serve(d: &Path) -> Server {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let port = line.split("localhost:").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    Server(c, port)
}

fn ask(port: u16, method: &str, path: &str, host: Option<&str>) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let host = host.map(str::to_string).unwrap_or(format!("localhost:{port}"));
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), body.to_string())
}

fn repo(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("flow-serve-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("app/api/pay")).unwrap();
    sh(&d, &["git", "init", "-q", "-b", "main"]);
    sh(&d, &["git", "config", "user.email", "t@t"]);
    sh(&d, &["git", "config", "user.name", "t"]);
    std::fs::write(d.join("app/api/pay/route.ts"), "export async function POST() {\n  return new Response('1');\n}\n").unwrap();
    sh(&d, &["git", "add", "-A"]);
    sh(&d, &["git", "commit", "-qm", "First"]);
    std::fs::write(d.join("app/api/pay/route.ts"), "export async function POST() {\n  return new Response('2');\n}\n").unwrap();
    sh(&d, &["git", "add", "-A"]);
    sh(&d, &["git", "commit", "-qm", "Second"]);
    d
}

#[test]
fn it_shows_a_change_and_refuses_the_rest() {
    let d = repo("shows");
    let before = sh(&d, &["git", "status", "--porcelain"]);
    let s = serve(&d);
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let head = sh(&d, &["git", "rev-parse", "HEAD"]);

    assert!(ask(s.1, "GET", "/", None).0.contains("200"));
    let (status, list) = ask(s.1, "GET", &format!("/api/p/{name}/commits"), None);
    assert!(status.contains("200") && list.contains("Second"), "{list}");
    let (status, one) = ask(s.1, "GET", &format!("/api/p/{name}/commit/{head}"), None);
    assert!(status.contains("200") && one.contains("POST /api/pay") && one.contains("app/api/pay/route.ts#POST"), "{one}");

    // Another site pointing its own name at this address cannot read the code.
    assert!(ask(s.1, "GET", "/api/projects", Some("evil.example")).0.contains("403"));
    assert!(ask(s.1, "POST", "/api/projects", None).0.contains("405"));
    // Only files in the commit, and only commit ids.
    assert!(ask(s.1, "GET", &format!("/api/p/{name}/file/{head}?path=..%2F..%2Fetc%2Fpasswd"), None).0.contains("400"));
    assert!(ask(s.1, "GET", &format!("/api/p/{name}/commit/--all"), None).0.contains("400"));
    assert!(ask(s.1, "GET", "/api/p/nobody/commits", None).0.contains("400"));

    assert_eq!(sh(&d, &["git", "status", "--porcelain"]), before, "serving must not touch the repo");
}

#[test]
fn a_journey_groups_its_requests_and_a_running_server_notices() {
    let d = repo("journeys");
    let s = serve(&d);
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let commits = format!("/api/p/{name}/commits");
    let (_, body) = ask(s.1, "GET", &commits, None);
    assert!(body.contains(r#""journeys":[]"#), "no file, no journeys: {body}");

    std::fs::write(d.join("flow.journeys.json"), r#"{"journeys":[{"title":"Pay for a stay","icon":"card","requests":["POST /api/pay"],"stops":[{"say":"Enter your card.","requests":["POST /api/pay"]},{"say":"Get a receipt.","requests":["POST /api/pay"]}]}]}"#).unwrap();
    let (_, body) = ask(s.1, "GET", &commits, None);
    assert!(body.contains("Pay for a stay") && body.contains("Enter your card."), "{body}");
    assert!(body.contains(r#""requests":["j_"#), "a stop names its requests by permanent id: {body}");
    let head = sh(&d, &["git", "rev-parse", "HEAD"]);
    let (_, sum) = ask(s.1, "GET", &format!("/api/p/{name}/summaries?ids={head}"), None);
    assert!(sum.contains(r#""group":0"#), "{sum}");
    assert!(sum.contains(r#""focus":[0]"#), "{sum}");
    let (_, requests) = ask(s.1, "GET", &format!("/api/p/{name}/requests"), None);
    assert!(requests.contains("POST /api/pay") && requests.contains(r#""group":0"#), "{requests}");
    let uid = requests.split(r#""uid":""#).nth(1).unwrap().split('"').next().unwrap();
    let (_, one) = ask(s.1, "GET", &format!("/api/p/{name}/request/{uid}"), None);
    assert!(one.contains(r#""steps":["#) && one.contains("route.ts"), "{one}");
    let (status, _) = ask(s.1, "GET", &format!("/api/p/{name}/request/j_nothing"), None);
    assert!(status.contains("400"), "{status}");
    let (_, activity) = ask(s.1, "GET", &format!("/api/p/{name}/activity"), None);
    assert_eq!(activity.matches(r#","t"]"#).count(), 2, "{activity}");
    let (_, detail) = ask(s.1, "GET", &format!("/api/p/{name}/commit/{head}"), None);
    assert!(detail.contains(r#""group":0"#), "{detail}");
    assert_eq!(sh(&d, &["git", "status", "--porcelain"]), "?? flow.journeys.json", "serving writes nothing");

    std::fs::write(d.join("flow.titles.json"), format!(r#"{{"{head}":{{"title":"Paying costs less."}}}}"#)).unwrap();
    let (_, body) = ask(s.1, "GET", &commits, None);
    assert!(body.contains("Paying costs less.") && body.contains(r#""subject":"Second""#), "{body}");
}

/// A page whose script does not parse is a blank page, however many endpoints work.
#[test]
fn the_page_script_parses() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let page = include_str!("../src/serve.html");
    let script = &page[page.find("<script>").unwrap() + 8..page.rfind("</script>").unwrap()];
    let path = std::env::temp_dir().join(format!("flow-page-{}.js", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let o = Command::new("node").arg("--check").arg(&path).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// Runs the server with a stand-in for the GitHub command line.
fn serve_with_gh(d: &Path, script: &str) -> Server {
    let bin = d.join(".fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, format!("#!/bin/sh\n{script}\n")).unwrap();
    sh(d, &["chmod", "+x", gh.to_str().unwrap()]);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", &port.to_string()]).current_dir(d).env("PATH", path).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    Server(c, port)
}

fn body(port: u16, path: &str) -> (String, serde_json::Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut t = String::new();
    s.read_to_string(&mut t).unwrap();
    let (head, b) = t.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), serde_json::from_str(b).unwrap())
}

#[test]
fn pull_requests_read_from_github_and_keep_what_is_missing_honest() {
    let dir = repo("pulls");
    let d = dir.as_path();
    let sha = sh(d, &["git", "rev-parse", "HEAD"]);
    let gone = "0123456789012345678901234567890123456789";
    let fields = r#""title":"Add a","isDraft":true,"state":"OPEN","author":{"login":"sam"},"createdAt":"2026-01-02T03:04:05Z","mergedAt":null,"closedAt":null,"baseRefName":"main","headRefName":"a","changedFiles":1,"url":"https://github.com/o/r/pull/3""#;
    let script = format!(
        "case \"$2\" in list) echo '[{{\"number\":3,{fields}}}]';; view) [ \"$3\" = 3 ] || exit 1; echo '{{\"number\":3,{fields},\"body\":\"Why\",\"commits\":[{{\"oid\":\"{sha}\"}},{{\"oid\":\"{gone}\"}}]}}';; esac"
    );
    let s = serve_with_gh(d, &script);
    let name = d.canonicalize().unwrap().file_name().unwrap().to_string_lossy().to_string();

    let (_, list) = body(s.1, &format!("/api/p/{name}/pulls"));
    assert_eq!(list["linked"], true);
    assert_eq!(list["pulls"][0]["state"], "draft", "GitHub calls a draft open");
    assert_eq!(list["pulls"][0]["opened"], "2026-01-02");

    let (_, one) = body(s.1, &format!("/api/p/{name}/pull/3"));
    assert_eq!(one["pull"]["body"], "Why");
    assert_eq!(one["total"], 2);
    assert_eq!(one["missing"], 1, "a commit that was never fetched is counted, not fetched");
    assert_eq!(one["commits"].as_array().unwrap().len(), 1);

    let (status, _) = body(s.1, &format!("/api/p/{name}/pull/3;rm"));
    assert!(status.contains("400"), "{status}");
}

#[test]
fn a_project_off_github_says_so_and_does_not_fail() {
    let dir = repo("nogithub");
    let d = dir.as_path();
    let s = serve_with_gh(d, "echo 'no remotes' >&2; exit 1");
    let name = d.canonicalize().unwrap().file_name().unwrap().to_string_lossy().to_string();
    let (status, list) = body(s.1, &format!("/api/p/{name}/pulls"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(list["linked"], false);
}

/// A stand-in for GitHub that signs in whoever `login` names and writes down what it was sent.
fn fake_github(login: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s);
            let (mut first, mut len, mut auth) = (String::new(), 0usize, String::new());
            r.read_line(&mut first).unwrap();
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let low = h.to_lowercase();
                if let Some(v) = low.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if low.starts_with("authorization:") {
                    auth = h.trim().to_string();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            log.lock().unwrap().push(format!("{} {} {}", first.trim(), auth, String::from_utf8_lossy(&body)));
            let out = if first.contains("access_token") {
                r#"{"access_token":"gho_test"}"#.to_string()
            } else {
                format!(r#"{{"login":"{login}","name":"Sam Test","avatar_url":"https://avatars.example/1"}}"#)
            };
            write!(r.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).unwrap();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn locked(d: &Path, github: &str, allowed: &str) -> Server {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).env("FLOW_GITHUB_CLIENT_ID", "the-id").env("FLOW_GITHUB_CLIENT_SECRET", "the-secret").env("FLOW_ALLOWED_USERS", allowed).env("FLOW_GITHUB_URL", github).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    assert!(line.contains("sign-in required"), "{line}");
    let port = line.split("localhost:").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    Server(c, port)
}

/// Status line, every header, body.
fn req(port: u16, method: &str, path: &str, more: &str) -> (String, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{more}Connection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let (first, rest) = head.split_once("\r\n").unwrap();
    (first.to_string(), rest.to_string(), body.to_string())
}

fn header<'a>(head: &'a str, name: &str, starts: &str) -> Option<&'a str> {
    head.lines().filter_map(|l| l.split_once(": ")).find(|(k, v)| k.eq_ignore_ascii_case(name) && v.starts_with(starts)).map(|(_, v)| v)
}

/// Starts a sign-in and comes back from GitHub with `code`. Returns the callback's headers.
fn sign_in(port: u16) -> String {
    let (_, head, _) = req(port, "GET", "/auth/github", "");
    let to = header(&head, "Location", "").unwrap();
    let state = to.split("state=").nth(1).unwrap().to_string();
    let (status, head, _) = req(port, "GET", &format!("/auth/github/callback?code=abc123&state={state}"), &format!("Cookie: flow_oauth={state}\r\n"));
    assert!(status.contains("302"), "{status}");
    head
}

#[test]
fn signing_in_locks_the_code_until_github_says_who_you_are() {
    let d = repo("signin");
    let (github, seen) = fake_github("sam");
    let s = locked(&d, &github, "Sam, other");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let code = format!("/api/p/{name}/commits");

    // The page loads so it can offer sign-in. Nothing with the code in it does.
    assert!(req(s.1, "GET", "/", "").0.contains("200"));
    for path in ["/api/projects", code.as_str()] {
        assert!(req(s.1, "GET", path, "").0.contains("401"), "{path}");
        assert!(req(s.1, "GET", path, "Cookie: flow_session=guessed\r\n").0.contains("401"), "{path}");
    }
    let (_, _, me) = req(s.1, "GET", "/api/me", "");
    assert!(me.contains(r#""required":true"#) && me.contains(r#""user":null"#), "{me}");

    let (status, head, _) = req(s.1, "GET", "/auth/github", "");
    let to = header(&head, "Location", "").unwrap();
    assert!(status.contains("302") && to.contains("client_id=the-id") && to.contains("localhost%3A") && !to.contains("the-secret"), "{to}");
    assert!(!to.contains("scope="), "it must ask GitHub for nothing");

    let head = sign_in(s.1);
    let cookie = header(&head, "Set-Cookie", "flow_session=").expect("a session");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"), "{cookie}");
    let token = cookie.split(';').next().unwrap().to_string();
    let sent = format!("Cookie: {token}\r\n");
    assert!(req(s.1, "GET", &code, &sent).0.contains("200"));
    let (_, _, me) = req(s.1, "GET", "/api/me", &sent);
    assert!(me.contains("Sam Test") && !me.contains("gho_test"), "{me}");
    let told = seen.lock().unwrap().join("\n");
    assert!(told.contains("client_secret=the-secret") && told.contains("Bearer gho_test"), "{told}");

    // Another site cannot sign you out, and this page can.
    assert!(req(s.1, "POST", "/auth/logout", &format!("{sent}Origin: http://evil.example\r\n")).0.contains("403"));
    assert!(req(s.1, "GET", &code, &sent).0.contains("200"));
    assert!(req(s.1, "POST", "/auth/logout", &format!("{sent}Origin: http://localhost:{}\r\n", s.1)).0.contains("200"));
    assert!(req(s.1, "GET", &code, &sent).0.contains("401"), "the session must be gone, not just the cookie");
}

#[test]
fn a_forged_or_repeated_sign_in_gets_nothing() {
    let d = repo("forged");
    let (github, seen) = fake_github("sam");
    let s = locked(&d, &github, "sam");

    let (_, head, _) = req(s.1, "GET", "/auth/github", "");
    let state = header(&head, "Location", "").unwrap().split("state=").nth(1).unwrap().to_string();
    let callback = format!("/auth/github/callback?code=abc&state={state}");
    // Someone else's link: the state is real but this browser did not start it.
    let (_, head, _) = req(s.1, "GET", &callback, "");
    assert!(header(&head, "Location", "/?signin=failed").is_some() && header(&head, "Set-Cookie", "flow_session").is_none(), "{head}");
    // That spent it, and the browser that did start it cannot use it after.
    let (_, head, _) = req(s.1, "GET", &callback, &format!("Cookie: flow_oauth={state}\r\n"));
    assert!(header(&head, "Set-Cookie", "flow_session").is_none(), "{head}");
    assert!(seen.lock().unwrap().is_empty(), "nothing forged may reach GitHub");

    // A code that could not have come from GitHub does not reach curl.
    let (_, head, _) = req(s.1, "GET", "/auth/github", "");
    let state = header(&head, "Location", "").unwrap().split("state=").nth(1).unwrap().to_string();
    let (_, head, _) = req(s.1, "GET", &format!("/auth/github/callback?code=a%22%0Aurl%3Devil&state={state}"), &format!("Cookie: flow_oauth={state}\r\n"));
    assert!(header(&head, "Location", "/?signin=failed").is_some(), "{head}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn only_the_people_named_may_sign_in() {
    let d = repo("denied");
    let (github, _) = fake_github("stranger");
    let s = locked(&d, &github, "sam");
    let head = sign_in(s.1);
    assert!(header(&head, "Location", "/?signin=denied").is_some() && header(&head, "Set-Cookie", "flow_session").is_none(), "{head}");
}

#[test]
fn half_a_sign_in_setup_refuses_to_start_open() {
    let d = repo("half");
    let run = |vars: &[(&str, &str)]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flow"));
        c.args(["serve", "--port", "0"]).current_dir(&d).stdout(Stdio::null()).stderr(Stdio::piped());
        for (k, v) in vars {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    let o = run(&[("FLOW_GITHUB_CLIENT_ID", "x")]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("FLOW_GITHUB_CLIENT_SECRET"));
    let o = run(&[("FLOW_GITHUB_CLIENT_ID", "x"), ("FLOW_GITHUB_CLIENT_SECRET", "y")]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("FLOW_ALLOWED_USERS"));
}
