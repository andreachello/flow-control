//! Running journeys for real: the $192 merge, found by running it.

use flowcontrol::merge;
use flowcontrol::names::Names;
use flowcontrol::scenario::{self, Kind};
use flowcontrol::vcs::Vcs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(d: &Path, a: &[&str]) {
    let o = Command::new("git").arg("-C").arg(d).args(a).output().unwrap();
    assert!(o.status.success(), "git {a:?}: {}", String::from_utf8_lossy(&o.stderr));
}
fn write(d: &Path, p: &str, t: &str) {
    let f = d.join(p);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, t).unwrap();
}
fn edit(d: &Path, p: &str, a: &str, b: &str) {
    let s = std::fs::read_to_string(d.join(p)).unwrap();
    assert!(s.contains(a));
    std::fs::write(d.join(p), s.replacen(a, b, 1)).unwrap();
}
fn open(d: &Path) -> Vcs {
    Vcs::open(d, "flows", Names::default()).unwrap()
}

const BILLING: &str = "export function price(interval) {\n  return 24;\n}\n\nexport function charge(amount) {\n  return amount;\n}\n";
const ROUTE: &str = "import { price, charge } from \"../../../lib/billing.mjs\";\n\nexport async function POST() {\n  return Response.json({ charged: charge(price(\"year\")) });\n}\n";
const SCENARIO: &str = "import { price, charge } from \"../lib/billing.mjs\";\nconsole.log(\"booting\");\nconsole.log(JSON.stringify({ charged: charge(price(\"year\")), at: process.hrtime.bigint().toString() }));\n";
const TOML: &str = "[[scenario]]\nname = \"upgrade\"\njourney = \"POST /api/upgrade\"\nrun = \"node scenarios/upgrade.mjs\"\n";

fn demo(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    write(&d, "lib/billing.mjs", BILLING);
    write(&d, "app/api/upgrade/route.mjs", ROUTE);
    write(&d, "scenarios/upgrade.mjs", SCENARIO);
    write(&d, "flow.toml", TOML);
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "loyalty"]);
    edit(&d, "lib/billing.mjs", "return amount;", "return amount * 0.8;");
    open(&d).save("Loyalty discount").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    edit(&d, "lib/billing.mjs", "return 24;", "return interval === \"year\" ? 240 : 24;");
    open(&d).save("Annual plans").unwrap();
    d
}

#[test]
fn a_clean_text_merge_is_caught_by_running_it() {
    let d = demo("sc-192");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "loyalty").unwrap();
    assert!(p.collisions.is_empty());
    assert_eq!(p.meet_ids, ["POST /api/upgrade"]);
    let checks = scenario::check_merge(&v, &p, &p.base).unwrap();
    assert_eq!(checks.len(), 1);
    let c = &checks[0];
    assert_eq!(c.unstable, ["at"], "a timestamp is not evidence");
    let f = c.findings.iter().find(|f| f.key == "charged").unwrap();
    assert_eq!(f.kind, Kind::BothDecide);
    assert_eq!((f.base.as_deref(), f.ours.as_deref(), f.theirs.as_deref(), f.merged.as_deref()), (Some("24"), Some("240"), Some("19.200000000000003"), Some("192")));

    // Saving is blocked until a person decides.
    merge::apply_checked(&v, &p, "loyalty", checks).unwrap();
    assert!(v.save("Merge").unwrap_err().to_string().contains("nobody has accepted"));
    merge::accept(&v, "upgrade").unwrap();
    let mut v = open(&d);
    v.save("Merge, 192 is right").unwrap();
    assert_eq!(scenario::recorded(&d, "flows", "upgrade").unwrap()["charged"], "192");
    // No stray checkouts left behind.
    assert!(!String::from_utf8_lossy(&Command::new("git").arg("-C").arg(&d).args(["worktree", "list"]).output().unwrap().stdout).contains("flow-tree"));
}

/// Start the $192 merge, so saving is blocked.
fn blocked(name: &str) -> (PathBuf, Vcs) {
    let d = demo(name);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "loyalty").unwrap();
    let checks = scenario::check_merge(&v, &p, &p.base).unwrap();
    merge::apply_checked(&v, &p, "loyalty", checks).unwrap();
    assert!(v.save("Merge").unwrap_err().to_string().contains("nobody has accepted"));
    (d, v)
}

fn worktrees(d: &Path) -> String {
    String::from_utf8_lossy(&Command::new("git").arg("-C").arg(d).args(["worktree", "list"]).output().unwrap().stdout).to_string()
}

#[test]
fn fixing_the_code_during_a_merge_unblocks_it() {
    let (d, mut v) = blocked("sc-fix");
    // Drop the discount: the merge now charges what this side charges, so the two sides are settled.
    edit(&d, "lib/billing.mjs", "return amount * 0.8;", "return amount;");
    v.save("Merge, no discount on annual").unwrap();
    assert!(!worktrees(&d).contains("flow-tree"), "no stray checkouts");
    assert!(!v.merge_state_path().exists());
}

#[test]
fn an_edit_that_changes_nothing_stays_blocked() {
    let (d, mut v) = blocked("sc-noop");
    write(&d, "lib/billing.mjs", &(std::fs::read_to_string(d.join("lib/billing.mjs")).unwrap() + "\n// A comment.\n"));
    assert!(v.save("Merge").unwrap_err().to_string().contains("nobody has accepted"));
}

#[test]
fn accepting_records_what_the_code_does_now_not_what_the_merge_did() {
    let (d, mut v) = blocked("sc-accept-now");
    edit(&d, "lib/billing.mjs", "return amount * 0.8;", "return amount * 0.5;");
    assert!(v.save("Merge").is_err(), "120 is neither side, so someone must still decide");
    merge::accept(&v, "upgrade").unwrap();
    v.save("Merge, half price").unwrap();
    assert_eq!(scenario::recorded(&d, "flows", "upgrade").unwrap()["charged"], "120");
}

#[test]
fn changing_the_code_after_accepting_takes_the_acceptance_back() {
    let (d, mut v) = blocked("sc-accept-stale");
    merge::accept(&v, "upgrade").unwrap();
    assert_eq!(scenario::recorded(&d, "flows", "upgrade").unwrap()["charged"], "192");
    edit(&d, "lib/billing.mjs", "return amount * 0.8;", "return amount * 0.5;");
    let e = v.save("Merge").unwrap_err().to_string();
    assert!(e.contains("nobody has accepted"), "{e}");
    assert!(scenario::recorded(&d, "flows", "upgrade").is_none(), "192 no longer describes the code");
    // Accepting a result the code does not change keeps standing.
    let (d, mut v) = blocked("sc-accept-keep");
    merge::accept(&v, "upgrade").unwrap();
    write(&d, "notes.txt", "Unrelated.\n");
    v.save("Merge").unwrap();
    assert_eq!(scenario::recorded(&d, "flows", "upgrade").unwrap()["charged"], "192");
}

#[test]
fn undoing_a_merge_takes_back_what_was_accepted_in_it() {
    let (d, v) = blocked("sc-accept-abort");
    merge::accept(&v, "upgrade").unwrap();
    merge::abort(&v).unwrap();
    assert!(scenario::recorded(&d, "flows", "upgrade").is_none());
}

#[test]
fn a_merge_that_breaks_can_be_repaired_in_place() {
    let d = broken("sc-repair");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "one").unwrap();
    let c = scenario::check_merge(&v, &p, &p.base).unwrap();
    merge::apply_checked(&v, &p, "one", c).unwrap();
    assert!(v.save("Merge").is_err());
    edit(&d, "s.mjs", "if (v === 30)", "if (v === 31)");
    v.save("Merge, repaired").unwrap();
}

/// Two branches that each work, and fail when merged.
fn broken(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    let lib = "export function a() {\n  return 1;\n}\n\nexport function b() {\n  return 2;\n}\n";
    write(&d, "lib/x.mjs", lib);
    write(&d, "app/api/go/route.mjs", "import { a, b } from \"../../../lib/x.mjs\";\n\nexport async function POST() {\n  return Response.json({ v: a() + b() });\n}\n");
    write(&d, "s.mjs", "import { a, b } from \"./lib/x.mjs\";\nconst v = a() + b();\nif (v === 30) { console.error(\"boom\"); process.exit(3); }\nconsole.log(JSON.stringify({ ok: true }));\n");
    write(&d, "flow.toml", "[[scenario]]\nname = \"go\"\nrun = \"node s.mjs\"\n");
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "one"]);
    edit(&d, "lib/x.mjs", "return 1;", "return 10;");
    open(&d).save("a").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    edit(&d, "lib/x.mjs", "return 2;", "return 20;");
    open(&d).save("b").unwrap();
    d
}

#[test]
fn a_merge_that_breaks_when_neither_side_did_is_caught() {
    let d = broken("sc-broken");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "one").unwrap();
    let c = scenario::check_merge(&v, &p, &p.base).unwrap();
    assert!(c[0].findings.iter().any(|f| f.kind == Kind::Broken && f.key == "failed"), "{:?}", c[0].findings);
}

#[test]
fn scenarios_can_be_any_language_and_plain_text() {
    assert_eq!(scenario::read_outcome("hello\n")["output"], "hello");
    let o = scenario::read_outcome("{\"a\":{\"b\":1},\"c\":\"x\"}");
    assert_eq!(o["a.b"], "1");
    assert_eq!(o["c"], "\"x\"");
    let same = scenario::judge("k", Some(&"1".into()), Some(&"2".into()), Some(&"1".into()), Some(&"2".into()));
    assert!(same.is_none(), "one side's change carries through");
    let new = scenario::judge("k", Some(&"1".into()), Some(&"1".into()), Some(&"1".into()), Some(&"5".into()));
    assert_eq!(new.unwrap().kind, Kind::OnlyTogether);
}
