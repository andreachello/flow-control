//! `flow serve` shows source code, so what it refuses matters as much as what it shows.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new(args[0]).args(&args[1..]).current_dir(dir).env("FLOW_CACHE", dir.join(".git/flow-cache")).output().unwrap();
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
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).env("FLOW_CACHE", d.join(".git/flow-cache")).stdout(Stdio::piped()).spawn().unwrap();
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
fn a_change_once_read_is_kept_through_a_restart_and_a_regrouping() {
    let d = repo("kept");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let head = sh(&d, &["git", "rev-parse", "HEAD"]);
    let at = format!("/api/p/{name}/summaries?ids={head}");
    let first = serve(&d);
    ask(first.1, "GET", &at, None);
    drop(first);
    // Marked by hand, so an answer with this in it can only have come from the file.
    let mut dirs = vec![d.join(".git/flow-cache")];
    let mut kept = None;
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                dirs.push(e.path());
            } else if e.file_name().to_string_lossy() == format!("{head}.json") {
                kept = Some(e.path());
            }
        }
    }
    let kept = kept.expect("a change that was read is on disk");
    let text = std::fs::read_to_string(&kept).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["untouched"] = 777.into();
    std::fs::write(&kept, v.to_string()).unwrap();

    let again = serve(&d);
    let (_, body) = ask(again.1, "GET", &at, None);
    assert!(body.contains(r#""untouched":777"#), "{body}");
    assert!(body.contains(r#""group":null"#), "{body}");
    std::fs::write(d.join("flow.journeys.json"), r#"{"journeys":[{"title":"Pay","icon":"card","requests":["POST /api/pay"]}]}"#).unwrap();
    ask(again.1, "GET", &format!("/api/p/{name}/commits"), None);
    let (_, body) = ask(again.1, "GET", &at, None);
    assert!(body.contains(r#""untouched":777"#) && body.contains(r#""group":0"#), "journeys are applied to what was kept: {body}");
    let nobody = "0".repeat(40);
    let (_, body) = ask(again.1, "GET", &format!("/api/p/{name}/summaries?saved=1&ids={head},{nobody}"), None);
    assert!(body.contains(r#""untouched":777"#) && !body.contains(&nobody), "only what is known comes back, and nothing is worked out: {body}");
}

#[test]
fn a_repo_of_several_products_reads_by_product() {
    let d = repo("products");
    let s = serve(&d);
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let file = r#"{"products":[{"name":"Shop","path":"app"},{"name":"Empty","path":"nowhere"}],"journeys":[{"product":"Shop","title":"Pay","icon":"card","requests":["POST /api/pay"]}],"maps":{"Shop":{"say":"People pay.","paths":[{"who":"Guests","journeys":["Pay"]}]}}}"#;
    std::fs::write(d.join("flow.journeys.json"), file).unwrap();
    let (_, body) = ask(s.1, "GET", &format!("/api/p/{name}/commits"), None);
    assert!(body.contains(r#""product":"Shop""#) && body.contains("People pay."), "{body}");
    assert!(body.contains(r#""products":["Shop"]"#), "a folder with no journeys is nothing to pick: {body}");
    let (_, requests) = ask(s.1, "GET", &format!("/api/p/{name}/requests"), None);
    assert!(requests.contains(r#""product":"Shop""#), "{requests}");

    std::fs::write(d.join("flow.journeys.json"), file.replace(r#""journeys":[{"#, r#""ignore":["app"],"journeys":[{"#)).unwrap();
    let (_, requests) = ask(s.1, "GET", &format!("/api/p/{name}/requests"), None);
    assert_eq!(requests, "[]", "an ignored folder is left out of the list");
}

#[test]
fn a_journey_groups_its_requests_and_a_running_server_notices() {
    let d = repo("journeys");
    let s = serve(&d);
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let commits = format!("/api/p/{name}/commits");
    let (_, body) = ask(s.1, "GET", &commits, None);
    assert!(body.contains(r#""journeys":[]"#), "no file, no journeys: {body}");
    let (_, requests) = ask(s.1, "GET", &format!("/api/p/{name}/requests"), None);
    assert!(requests.contains(r#""group":null"#), "read once before there are journeys, as a run does: {requests}");

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

#[test]
fn no_page_function_is_quietly_replaced_by_another() {
    let page = include_str!("../src/serve.html");
    let mut seen = std::collections::BTreeSet::new();
    for l in page.lines() {
        let Some(rest) = l.strip_prefix("async function ").or(l.strip_prefix("function ")) else { continue };
        let name = rest.split('(').next().unwrap();
        assert!(seen.insert(name), "two functions are called {name}, and only the last one runs");
    }
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
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", &port.to_string()]).current_dir(d).env("FLOW_CACHE", d.join(".git/flow-cache")).env("PATH", path).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
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
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).env("FLOW_CACHE", d.join(".git/flow-cache")).env("FLOW_GITHUB_CLIENT_ID", "the-id").env("FLOW_GITHUB_CLIENT_SECRET", "the-secret").env("FLOW_ALLOWED_USERS", allowed).env("FLOW_GITHUB_URL", github).stdout(Stdio::piped()).spawn().unwrap();
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
    let origin = format!("Origin: http://localhost:{}\r\n", s.1);
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/journeys"), &origin).0.contains("401"), "a run starts work, so it needs a signed-in person");
    assert!(req(s.1, "GET", &format!("/api/p/{name}/run"), "").0.contains("401"));
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
        c.args(["serve", "--port", "0"]).current_dir(&d).env("FLOW_CACHE", d.join(".git/flow-cache")).stdout(Stdio::null()).stderr(Stdio::piped());
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

/// Stands in for Claude on the PATH, answering the two questions `flow journeys` asks.
fn fake_claude(dir: &Path, script: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let f = dir.join("claude");
    std::fs::write(&f, format!("#!/bin/sh\ncat >/dev/null\n{script}\n")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", dir.display(), std::env::var("PATH").unwrap())
}

fn serve_on(d: &Path, path: &str) -> Server {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).env("FLOW_CACHE", d.join(".git/flow-cache")).env("PATH", path).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let port = line.split("localhost:").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    Server(c, port)
}

fn settled(port: u16, at: &str) -> String {
    for _ in 0..1500 {
        let (_, _, body) = req(port, "GET", at, "");
        if body.contains(r#""running":false"#) {
            return body;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the run never ended");
}

#[test]
fn the_page_can_find_journeys_and_nothing_else_can_ask() {
    let d = repo("run");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let path = fake_claude(&d.with_extension("bin"), r#"case "$*" in
  *'"paths"'*) echo '{"structured_output":{"say":"People pay for stays.","paths":[{"who":"Guests","journeys":["Pay for a stay"]}]}}' ;;
  *'"stops"'*) echo '{"structured_output":{"stops":[{"say":"Enter your card.","requests":["POST /api/pay"]},{"say":"Get a receipt.","requests":["POST /api/pay"]}]}}' ;;
  *) echo '{"structured_output":{"journeys":[{"title":"Pay for a stay","icon":"card","requests":["POST /api/pay"]}]}}' ;;
esac"#);
    let s = serve_on(&d, &path);
    let (start, watch) = (format!("/api/p/{name}/run/journeys"), format!("/api/p/{name}/run"));
    let ours = format!("Origin: http://localhost:{}\r\n", s.1);

    for refused in ["", "Origin: http://evil.example\r\n", "Origin: null\r\n"] {
        assert!(req(s.1, "POST", &start, refused).0.contains("403"), "{refused:?}");
    }
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/lift"), &ours).0.contains("400"), "not on the list");
    assert!(req(s.1, "POST", "/api/p/nobody/run/journeys", &ours).0.contains("400"));
    assert!(req(s.1, "GET", &start, "").0.contains("400"), "reading the address starts nothing");
    assert_eq!(req(s.1, "GET", &watch, "").2, r#"{"run":null}"#, "nothing refused may have started");

    let (status, _, body) = req(s.1, "POST", &start, &ours);
    assert!(status.contains("200") && body.contains(r#""task":"journeys""#), "{body}");
    let done = settled(s.1, &watch);
    assert!(done.contains(r#""failed":null"#), "{done}");
    assert!(done.contains(r#""step":"Writing out journeys, 1 of 1 done""#), "where it got to reaches the page: {done}");
    let (_, _, list) = req(s.1, "GET", &format!("/api/p/{name}/commits"), "");
    assert!(list.contains("Pay for a stay") && list.contains("Enter your card."), "the running page sees it: {list}");
}

#[test]
fn a_run_that_cannot_work_says_why() {
    let d = repo("run-fails");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let path = fake_claude(&d.with_extension("bin"), r#"echo '{"is_error":true,"result":"Not logged in"}'"#);
    let s = serve_on(&d, &path);
    let ours = format!("Origin: http://localhost:{}\r\n", s.1);
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/journeys"), &ours).0.contains("200"));
    let done = settled(s.1, &format!("/api/p/{name}/run"));
    assert!(done.contains("Not logged in"), "{done}");
    assert!(!done.contains("flow:"), "{done}");
}

/// Stands in for Soba: `runs` is what a run gets back, and every request is kept.
fn fake_soba(status: &'static str, runs: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::{Arc, Mutex};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s);
            let (mut first, mut auth, mut len) = (String::new(), String::new(), 0usize);
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
            log.lock().unwrap().push(format!("{} {auth} {}", first.trim(), String::from_utf8_lossy(&body)));
            let (st, kind, out) = if first.contains("/runs") { (status, if status.starts_with("200") { "text/event-stream" } else { "application/json" }, runs) } else if first.contains("/keys") { ("200 OK", "application/json", r#"{"keys":[]}"#) } else { ("200 OK", "application/json", r#"{"session":"est_1","expires_in":600,"url":"https://soba.so/link#est_1"}"#) };
            write!(r.get_mut(), "HTTP/1.1 {st}\r\nContent-Type: {kind}\r\nSoba-Run-Id: run_1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).unwrap();
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

/// Where the tests share one install of Soba's SDK, made once. None without Node, like the page test.
fn soba_home() -> Option<String> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        Command::new("node").arg("--version").output().ok()?;
        let home = format!("{}/soba-helper", env!("CARGO_TARGET_TMPDIR"));
        let d = repo("soba-install");
        let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(&d).env("FLOW_CACHE", d.join(".git/flow-cache")).env("SOBA_KEY", "sk_soba_test.x").env("FLOW_SOBA_HOME", &home).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        let _ = c.kill();
        assert!(line.contains("localhost:"), "the install failed");
        Some(home)
    }).clone()
}

fn locked_with_soba(d: &Path, github: &str, soba: &str, claude: &str, encrypted: bool) -> Server {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0"]).current_dir(d).env("FLOW_CACHE", d.join(".git/flow-cache"))
        .env("PATH", claude).env("FLOW_GITHUB_CLIENT_ID", "the-id").env("FLOW_GITHUB_CLIENT_SECRET", "the-secret").env("FLOW_ALLOWED_USERS", "Sam")
        .env("FLOW_GITHUB_URL", github)
        .env("SOBA_KEY", "sk_soba_test.secret").env("SOBA_BASE_URL", soba).env("SOBA_WEB_URL", soba).env("FLOW_SOBA_HOME", soba_home().unwrap())
        .env("FLOW_SOBA_ENCRYPT", if encrypted { "on" } else { "off" })
        .stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let port = line.split("localhost:").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    Server(c, port)
}

/// A claude that must never be asked, because the person is signed in and Soba is set up.
fn no_claude(d: &Path) -> String {
    fake_claude(&d.with_extension("bin"), r#"echo '{"structured_output":{"journeys":[{"title":"The local agent answered","icon":"card","requests":["POST /api/pay"]}]}}'"#)
}

#[test]
fn a_run_goes_through_soba_as_the_signed_in_person() {
    if soba_home().is_none() {
        return;
    }
    let d = repo("soba");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let (github, _) = fake_github("sam");
    let (soba, seen) = fake_soba("200 OK", "data: {\"type\":\"delta\",\"text\":\"{\\\"journeys\\\":[{\\\"title\\\":\\\"Pay through Soba\\\",\\\"icon\\\":\\\"card\\\",\\\"requests\\\":[\\\"POST /api/pay\\\"]}],\\\"stops\\\":[]}\"}\n\ndata: {\"type\":\"done\"}\n\n");
    let s = locked_with_soba(&d, &github, &soba, &no_claude(&d), false);
    let ours = format!("Origin: http://localhost:{}\r\n", s.1);

    // Before sign-in there is nobody to act for.
    assert!(req(s.1, "POST", "/api/soba/session", &ours).0.contains("401"));
    let token = header(&sign_in(s.1), "Set-Cookie", "flow_session=").unwrap().split(';').next().unwrap().to_string();
    let sent = format!("Cookie: {token}\r\n");
    assert!(req(s.1, "POST", "/api/soba/session", &sent).0.contains("403"), "no Origin, so not this page");
    assert!(req(s.1, "GET", "/api/me", &sent).2.contains(r#""soba":true"#));

    let (status, _, body) = req(s.1, "POST", "/api/soba/session", &format!("{sent}{ours}"));
    assert!(status.contains("200") && body.contains("soba.so/link"), "{body}");
    assert!(!body.contains("sk_soba"), "{body}");

    // The request names nobody: the id comes from the sign-in.
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/journeys"), &format!("{sent}{ours}")).0.contains("200"));
    let watch = format!("/api/p/{name}/run");
    let done = (0..1500).find_map(|_| { let (_, _, b) = req(s.1, "GET", &watch, &sent); std::thread::sleep(std::time::Duration::from_millis(20)); b.contains(r#""running":false"#).then_some(b) }).expect("it ended");
    let (_, _, list) = req(s.1, "GET", &format!("/api/p/{name}/commits"), &sent);
    assert!(list.contains("Pay through Soba") && !list.contains("local agent"), "{done} {list}");

    let told = seen.lock().unwrap().join("\n");
    // sobaUserId("sam"), from Soba's own SDK.
    let sam = "usr_e96e02d8e47f";
    assert!(told.contains(&format!(r#""user":"{sam}""#)) && told.contains("/end_users/session") && told.contains("/runs"), "{told}");
    assert!(told.contains("Bearer sk_soba_test.secret"), "{told}");
}

#[test]
fn a_run_nobody_can_open_never_leaves_this_server() {
    if soba_home().is_none() {
        return;
    }
    let d = repo("soba-sealed");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let (github, _) = fake_github("sam");
    let (soba, seen) = fake_soba("200 OK", "");
    let s = locked_with_soba(&d, &github, &soba, &no_claude(&d), true);
    let token = header(&sign_in(s.1), "Set-Cookie", "flow_session=").unwrap().split(';').next().unwrap().to_string();
    let sent = format!("Cookie: {token}\r\nOrigin: http://localhost:{}\r\n", s.1);
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/journeys"), &sent).0.contains("200"));
    let watch = format!("/api/p/{name}/run");
    let done = (0..1500).find_map(|_| { let (_, _, b) = req(s.1, "GET", &watch, &sent); std::thread::sleep(std::time::Duration::from_millis(20)); b.contains(r#""running":false"#).then_some(b) }).expect("it ended");
    assert!(done.contains(r#""code":"no_compute""#), "{done}");
    let told = seen.lock().unwrap().join("\n");
    assert!(told.contains("/keys") && !told.contains("/runs"), "no key to seal to, so nothing is sent: {told}");
    assert!(!d.join("flow.journeys.json").exists(), "{done}");
}

#[test]
fn a_refusal_reaches_the_page_with_its_code() {
    if soba_home().is_none() {
        return;
    }
    let d = repo("soba-refused");
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let (github, _) = fake_github("sam");
    let (soba, _) = fake_soba("409 Conflict", r#"{"error":{"type":"no_compute","message":"No compute reachable for usr_1.","code":"no_compute"}}"#);
    let s = locked_with_soba(&d, &github, &soba, &no_claude(&d), false);
    let token = header(&sign_in(s.1), "Set-Cookie", "flow_session=").unwrap().split(';').next().unwrap().to_string();
    let sent = format!("Cookie: {token}\r\nOrigin: http://localhost:{}\r\n", s.1);
    assert!(req(s.1, "POST", &format!("/api/p/{name}/run/journeys"), &sent).0.contains("200"));
    let watch = format!("/api/p/{name}/run");
    let done = (0..1500).find_map(|_| { let (_, _, b) = req(s.1, "GET", &watch, &sent); std::thread::sleep(std::time::Duration::from_millis(20)); b.contains(r#""running":false"#).then_some(b) }).expect("it ended");
    assert!(done.contains(r#""code":"no_compute""#), "{done}");
    assert!(!d.join("flow.journeys.json").exists(), "a refused run must not fall back to the local agent: {done}");
}

#[test]
fn the_key_comes_from_the_file_soba_wrote_and_never_from_a_project() {
    let Some(home) = soba_home() else { return };
    let (start, other) = (repo("soba-file"), repo("soba-other"));
    std::fs::write(other.join(".env.local"), "SOBA_KEY=sk_soba_other_app.x\n").unwrap();
    let (github, _) = fake_github("sam");
    let (soba, seen) = fake_soba("200 OK", "");
    let run = |dir: &Path, more: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flow"));
        c.args(["serve", "--port", "0"]).args(more).current_dir(dir).env("FLOW_CACHE", dir.join(".git/flow-cache")).env_remove("SOBA_KEY").env("FLOW_GITHUB_CLIENT_ID", "the-id").env("FLOW_GITHUB_CLIENT_SECRET", "the-secret").env("FLOW_ALLOWED_USERS", "sam").env("FLOW_GITHUB_URL", &github).env("SOBA_WEB_URL", &soba).env("FLOW_SOBA_HOME", &home).stdout(Stdio::piped());
        let mut c = c.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(c.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        let port = line.split("localhost:").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
        Server(c, port)
    };
    let connects = |s: &Server| {
        let token = header(&sign_in(s.1), "Set-Cookie", "flow_session=").unwrap().split(';').next().unwrap().to_string();
        req(s.1, "GET", "/api/me", &format!("Cookie: {token}\r\n")).2.contains(r#""soba":true"#)
    };

    // Another project's key is not this server's, even with that project on the page.
    let s = run(&start, &[other.to_str().unwrap()]);
    assert!(!connects(&s), "a project's .env.local must not be read");
    drop(s);

    std::fs::write(start.join(".env.local"), "SOBA_PROD_KEY=sk_soba_prod.x\nSOBA_KEY=sk_soba_dev.y\n").unwrap();
    let s = run(&start, &[]);
    assert!(connects(&s));
    let token = header(&sign_in(s.1), "Set-Cookie", "flow_session=").unwrap().split(';').next().unwrap().to_string();
    assert!(req(s.1, "POST", "/api/soba/session", &format!("Cookie: {token}\r\nOrigin: http://localhost:{}\r\n", s.1)).0.contains("200"));
    let told = seen.lock().unwrap().join("\n");
    assert!(told.contains("Bearer sk_soba_dev.y") && !told.contains("prod"), "{told}");
    drop(s);

    let o = Command::new(env!("CARGO_BIN_EXE_flow")).args(["serve", "--port", "0", "--env-file", "nowhere.env"]).current_dir(&start).env_remove("SOBA_KEY").output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("@soba-so/cli link"), "a named file that is missing must say so");
}
