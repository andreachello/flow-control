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
