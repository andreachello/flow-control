//! Permanent ids: a rename continues a journey's history, in every place the journey is named.

use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new(args[0]).args(&args[1..]).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn flow(dir: &Path, args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_flow")).args(args).current_dir(dir).env("FLOW_AI", "off").output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (good, out) = flow(dir, args);
    assert!(good, "flow {args:?}: {out}");
    out
}

fn write(dir: &Path, p: &str, text: &str) {
    let p = dir.join(p);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const CHARGE: &str = "export async function POST(req: Request) {\n  const body = await req.json();\n  const total = price(body.plan);\n  return Response.json({ total });\n}\n\nfunction price(plan: string) {\n  return plan === 'annual' ? 192 : 20;\n}\n";
const OTHER: &str = "export async function POST(req: Request) {\n  return Response.json({ ok: true, at: 'other' });\n}\n";

fn repo(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("flow-ids-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    sh(&d, &["git", "init", "-q", "-b", "main"]);
    sh(&d, &["git", "config", "user.email", "t@t"]);
    sh(&d, &["git", "config", "user.name", "t"]);
    write(&d, "app/api/charge/route.ts", CHARGE);
    write(&d, "app/api/other/route.ts", OTHER);
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "first"]);
    d
}

fn uid(d: &Path, journey: &str) -> String {
    let l = flowcontrol::lift(d, "flows").unwrap();
    l.flows.iter().find(|f| f.id == journey).unwrap_or_else(|| panic!("no {journey}")).uid.clone()
}

fn mv(d: &Path, from: &str, to: &str) {
    std::fs::create_dir_all(d.join(to).parent().unwrap()).unwrap();
    std::fs::rename(d.join(from), d.join(to)).unwrap();
}

#[test]
fn a_repo_that_renames_nothing_stores_nothing() {
    let d = repo("none");
    assert!(!d.join(flowcontrol::ids::FILE).exists());
    assert!(uid(&d, "POST /api/charge").starts_with("j_"));
}

#[test]
fn moving_a_route_keeps_its_id_and_its_history() {
    let d = repo("move");
    let id = uid(&d, "POST /api/charge");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    let out = ok(&d, &["lift"]);
    assert!(out.contains("POST /api/charge is now POST /api/pay"), "{out}");
    assert_eq!(uid(&d, "POST /api/pay"), id);
    ok(&d, &["check"]);
    ok(&d, &["save", "-m", "Move charge to pay"]);

    let log = ok(&d, &["history", "POST /api/pay"]);
    assert!(log.contains("journey first appears"), "{log}");
    assert!(log.contains("renamed from POST /api/charge"), "{log}");
    // The old name finds the same journey.
    assert_eq!(ok(&d, &["history", "POST /api/charge"]), log);
}

#[test]
fn a_rename_reads_as_one_change_not_a_loss_and_a_gain() {
    let d = repo("status");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    let out = ok(&d, &["status"]);
    assert!(out.contains("Renamed: POST /api/charge is now POST /api/pay"), "{out}");
    assert!(!out.contains("Journey removed"), "{out}");
    // Status only looks.
    assert!(!d.join(flowcontrol::ids::FILE).exists());
}

#[test]
fn renaming_a_function_keeps_the_step_id() {
    let d = repo("step");
    let l = flowcontrol::lift(&d, "flows").unwrap();
    let before = l.uids.step("app/api/charge/route.ts#price").to_string();
    write(&d, "app/api/charge/route.ts", &CHARGE.replace("price", "priceOf"));
    ok(&d, &["lift"]);
    let l = flowcontrol::lift(&d, "flows").unwrap();
    assert_eq!(l.uids.step("app/api/charge/route.ts#priceOf"), before);
}

#[test]
fn a_shared_name_alone_is_not_a_rename() {
    let d = repo("guess");
    let id = uid(&d, "POST /api/charge");
    std::fs::remove_file(d.join("app/api/charge/route.ts")).unwrap();
    write(&d, "app/api/refund/route.ts", "export async function POST() {\n  const done = await refundEverything();\n  return new Response(String(done));\n}\n");
    let out = ok(&d, &["lift"]);
    assert!(out.contains("flow rename"), "{out}");
    assert_ne!(uid(&d, "POST /api/refund"), id);

    // A person can say what the code could not prove.
    ok(&d, &["rename", "POST /api/charge", "POST /api/refund"]);
    assert_eq!(uid(&d, "POST /api/refund"), id);
    ok(&d, &["check"]);
}

#[test]
fn undoing_a_rename_before_saving_leaves_no_trace() {
    let d = repo("undo");
    let id = uid(&d, "POST /api/charge");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    assert!(d.join(flowcontrol::ids::FILE).exists());
    mv(&d, "app/api/pay/route.ts", "app/api/charge/route.ts");
    ok(&d, &["lift"]);
    assert_eq!(uid(&d, "POST /api/charge"), id);
    assert_eq!(std::fs::read_to_string(d.join(flowcontrol::ids::FILE)).unwrap_or_default(), "");
}

#[test]
fn a_new_journey_on_a_retired_name_is_a_new_journey() {
    let d = repo("reuse");
    let id = uid(&d, "POST /api/charge");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "Rename"]);
    write(&d, "app/api/charge/route.ts", "export async function POST() {\n  return new Response('brand new');\n}\n");
    ok(&d, &["lift"]);
    assert_eq!(uid(&d, "POST /api/pay"), id);
    assert_ne!(uid(&d, "POST /api/charge"), id);
}

#[test]
fn renaming_there_and_back_ends_where_it_began() {
    let d = repo("back");
    let id = uid(&d, "POST /api/charge");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "There"]);
    mv(&d, "app/api/pay/route.ts", "app/api/charge/route.ts");
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "Back"]);
    assert_eq!(uid(&d, "POST /api/charge"), id);
    let log = ok(&d, &["history", "POST /api/charge"]);
    assert_eq!(log.matches("renamed from").count(), 2, "{log}");
}

#[test]
fn a_decision_follows_its_journey() {
    let d = repo("decide");
    ok(&d, &["decide", "POST /api/charge", "yes", "Annual plans cost 192."]);
    sh(&d, &["git", "add", "-A"]);
    sh(&d, &["git", "commit", "-q", "-m", "Decide"]);
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    let out = ok(&d, &["comment", "HEAD"]);
    assert!(!out.contains("Touched"), "a move does not change what the journey does: {out}");
}

#[test]
fn both_sides_renaming_merge_the_same_either_way_round() {
    let tree = |first: &str, second: &str| {
        let d = repo(&format!("merge-{first}"));
        sh(&d, &["git", "checkout", "-q", "-b", "a"]);
        mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
        ok(&d, &["lift"]);
        ok(&d, &["save", "-m", "a"]);
        sh(&d, &["git", "checkout", "-q", "main"]);
        sh(&d, &["git", "checkout", "-q", "-b", "b"]);
        mv(&d, "app/api/other/route.ts", "app/api/elsewhere/route.ts");
        ok(&d, &["lift"]);
        ok(&d, &["save", "-m", "b"]);
        sh(&d, &["git", "checkout", "-q", first]);
        ok(&d, &["merge", second]);
        let text = std::fs::read_to_string(d.join(flowcontrol::ids::FILE)).unwrap();
        assert!(text.contains("POST /api/pay") && text.contains("POST /api/elsewhere"), "{text}");
        ok(&d, &["check"]);
        (text, uid(&d, "POST /api/pay"), uid(&d, "POST /api/elsewhere"))
    };
    assert_eq!(tree("a", "b"), tree("b", "a"));
}

#[test]
fn a_lift_of_a_commit_gives_the_same_ids_on_any_machine() {
    let d = repo("pure");
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "Rename"]);
    let copy = d.with_extension("clone");
    let _ = std::fs::remove_dir_all(&copy);
    sh(d.parent().unwrap(), &["git", "clone", "-q", d.to_str().unwrap(), copy.to_str().unwrap()]);
    let (a, b) = (flowcontrol::lift(&d, "flows").unwrap(), flowcontrol::lift(&copy, "flows").unwrap());
    assert_eq!(a.out.files, b.out.files);
}

#[test]
fn a_plain_name_follows_its_journey() {
    let d = repo("names");
    write(&d, "flow.names.json", r#"{"journeys":{"POST /api/charge":{"title":"Customer pays","summary":"","area":"Billing","hash":"x"}},"steps":{}}"#);
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    let n = flowcontrol::names::Names::load(&d.join("flow.names.json")).unwrap();
    assert_eq!(n.journeys["POST /api/pay"].title, "Customer pays");
}

#[test]
fn a_scenario_left_on_the_old_name_is_reported() {
    let d = repo("scenario");
    write(&d, "flow.toml", "[[scenario]]\nname = \"pay\"\njourney = \"POST /api/charge\"\nrun = \"true\"\n");
    ok(&d, &["lift"]);
    ok(&d, &["save", "-m", "Scenario"]);
    mv(&d, "app/api/charge/route.ts", "app/api/pay/route.ts");
    ok(&d, &["lift"]);
    let index = std::fs::read_to_string(d.join("flows/_index.yaml")).unwrap();
    assert!(index.contains("Update flow.toml"), "{index}");
}
