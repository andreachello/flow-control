//! Flow control on a real git repository built from the demo app.

use flowcontrol::merge;
use flowcontrol::names::Names;
use flowcontrol::vcs::Vcs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

fn edit(dir: &Path, file: &str, from: &str, to: &str) {
    let p = dir.join(file);
    let s = std::fs::read_to_string(&p).unwrap();
    assert!(s.contains(from), "{from} not in {file}");
    std::fs::write(&p, s.replacen(from, to, 1)).unwrap();
}

fn repo(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo");
    for e in walkdir::WalkDir::new(&src) {
        let e = e.unwrap();
        let rel = e.path().strip_prefix(&src).unwrap();
        if rel.starts_with("flows") {
            continue;
        }
        if e.file_type().is_dir() {
            std::fs::create_dir_all(d.join(rel)).unwrap();
        } else {
            std::fs::copy(e.path(), d.join(rel)).unwrap();
        }
    }
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    d
}

fn open(d: &Path) -> Vcs {
    Vcs::open(d, "flows", Names::default()).unwrap()
}

#[test]
fn status_and_history_speak_in_journeys() {
    let d = repo("vcs-history");
    open(&d).save("First").unwrap();
    edit(&d, "lib/billing.ts", "+ amount;", "+ Math.round(amount * 80) / 100;");
    let r = open(&d).status().unwrap();
    let ids: Vec<&str> = r.journeys.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(ids, ["POST /api/upgrade"]);
    assert_eq!(r.untouched, 5);
    assert_eq!(r.functions.len(), 1);
    open(&d).save("Loyalty discount").unwrap();
    let mut v = open(&d);
    let cs = v.commits(10, "HEAD").unwrap();
    assert_eq!(cs.len(), 2);
    let last = v.change_of(&cs[0]).unwrap();
    assert_eq!(flowcontrol::change::one_line(&last), "POST /api/upgrade");
}

#[test]
fn merging_by_function_finds_meets_and_collisions() {
    let d = repo("vcs-merge");
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "annual"]);
    edit(&d, "lib/billing.ts", "  return table[plan];", "  return table[plan] * (S.interval === \"year\" ? 10 : 1);");
    open(&d).save("Annual plans").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    git(&d, &["checkout", "-q", "-b", "loyalty"]);
    edit(&d, "lib/billing.ts", "+ amount;", "+ Math.round(amount * 80) / 100;");
    open(&d).save("Loyalty").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    git(&d, &["checkout", "-q", "-b", "rival"]);
    edit(&d, "lib/billing.ts", "  return table[plan];", "  return Math.round(table[plan] * 0.9);");
    open(&d).save("Ten percent off").unwrap();
    git(&d, &["checkout", "-q", "main"]);

    // Annual first: main has not moved, so it is taken whole.
    let mut v = open(&d);
    let p = merge::plan(&mut v, "annual").unwrap();
    assert!(p.fast_forward);
    merge::apply(&v, &p, "annual").unwrap();

    // Loyalty changes a different function: a clean merge that meets annual in the upgrade journey.
    let mut v = open(&d);
    let p = merge::plan(&mut v, "loyalty").unwrap();
    assert!(p.collisions.is_empty());
    assert_eq!(p.meets.len(), 1);
    assert_eq!(p.meets[0].0, "POST /api/upgrade");
    assert_eq!(p.meets[0].1, ["Resolve price"]);
    assert_eq!(p.meets[0].2, ["Charge card"]);
    merge::apply(&v, &p, "loyalty").unwrap();
    let billing = std::fs::read_to_string(d.join("lib/billing.ts")).unwrap();
    assert!(billing.contains("? 10 : 1)") && billing.contains("amount * 80"));
    v.save("Merge loyalty").unwrap();

    // Rival changes the same function as annual: a collision, kept as ours until decided.
    let mut v = open(&d);
    let p = merge::plan(&mut v, "rival").unwrap();
    assert_eq!(p.collisions.len(), 1);
    assert_eq!(p.collisions[0].label, "Resolve price");
    assert_eq!(p.collisions[0].journeys, ["POST /api/upgrade"]);
    merge::apply(&v, &p, "rival").unwrap();
    assert!(v.save("too early").is_err());
    merge::resolve(&v, "resolve price", "theirs").unwrap();
    let billing = std::fs::read_to_string(d.join("lib/billing.ts")).unwrap();
    assert!(billing.contains("* 0.9") && !billing.contains("? 10 : 1)") && billing.contains("amount * 80"));
    let mut v = open(&d);
    v.save("Merge rival").unwrap();
    assert_eq!(v.commits(1, "HEAD").unwrap()[0].parents.len(), 2);
}

#[test]
fn merging_keeps_imports_from_both_sides() {
    let ours = "import { a } from \"./a\";\n\nexport function f() {\n  return a();\n}\n";
    let base = ours;
    let theirs = "import { a } from \"./a\";\nimport { b } from \"./b\";\n\nexport function f() {\n  return a();\n}\n\nexport function g() {\n  return b();\n}\n";
    let ours2 = "import { a } from \"./a\";\nimport { c } from \"./c\";\n\nexport function f() {\n  return a() + c();\n}\n";
    let m = merge::merge_file("x.ts", Some(base), ours2, theirs).unwrap();
    let t = m.text.unwrap();
    assert!(t.contains("import { b }") && t.contains("import { c }") && t.contains("a() + c()") && t.contains("function g()"), "{t}");
    assert!(m.collisions.is_empty());
    let _ = ours;
}
