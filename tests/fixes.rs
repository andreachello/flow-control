//! The edge cases git handles badly, or flow used to.

use flowcontrol::merge::{self, merge_file};
use flowcontrol::names::Names;
use flowcontrol::source::Source;
use flowcontrol::vcs::Vcs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    for (p, t) in files {
        let f = d.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    d
}

fn write(d: &Path, p: &str, t: &str) {
    let f = d.join(p);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, t).unwrap();
}

fn open(d: &Path) -> Vcs {
    Vcs::open(d, "flows", Names::default()).unwrap()
}

const PRICE: &str = "export function price(n: number) {\n  const base = 20;\n  const tax = 0.2;\n  return base * n * (1 + tax);\n}\n";
const ROUTE: &str = "import { price } from \"../../../lib/price\";\n\nexport async function POST() {\n  return Response.json({ total: price(1) });\n}\n";

#[test]
fn reformatting_is_not_a_behaviour_change() {
    let d = repo("fix-format", &[("lib/price.ts", PRICE), ("app/api/buy/route.ts", ROUTE)]);
    open(&d).save("First").unwrap();
    write(&d, "lib/price.ts", "export function price(n: number) {\n    // the list price\n    const base = 20;\n    const tax = 0.2;\n\n    return base * n * (1 + tax);\n}\n");
    let r = open(&d).status().unwrap();
    assert!(r.journeys.is_empty(), "reformatting changed a journey: {:?}", r.journeys);
    assert_eq!(r.functions.len(), 1);
    assert_eq!(r.functions[0].state, "reformatted only");
}

#[test]
fn edits_to_different_lines_of_one_function_combine() {
    let ours = PRICE.replace("const base = 20;", "const base = 24;");
    let theirs = PRICE.replace("const tax = 0.2;", "const tax = 0.22;");
    let m = merge_file("lib/price.ts", Some(PRICE), &ours, &theirs).unwrap();
    assert!(m.collisions.is_empty());
    assert_eq!(m.combined, ["price"]);
    let t = m.text.unwrap();
    assert!(t.contains("const base = 24;") && t.contains("const tax = 0.22;"), "{t}");
}

#[test]
fn edits_to_the_same_line_collide() {
    let ours = PRICE.replace("const base = 20;", "const base = 24;");
    let theirs = PRICE.replace("const base = 20;", "const base = 18;");
    let m = merge_file("lib/price.ts", Some(PRICE), &ours, &theirs).unwrap();
    assert_eq!(m.collisions.len(), 1);
    assert!(m.combined.is_empty());
}

#[test]
fn one_side_reformats_the_other_changes_behaviour() {
    let ours = PRICE.replace("  const base = 20;\n", "    const base = 20;\n\n");
    let theirs = PRICE.replace("const tax = 0.2;", "const tax = 0.25;");
    let m = merge_file("lib/price.ts", Some(PRICE), &ours, &theirs).unwrap();
    assert!(m.collisions.is_empty() && m.combined.is_empty(), "{:?}", m.collisions);
    assert!(m.text.unwrap().contains("0.25"));
}

#[test]
fn files_that_are_not_code_merge_as_text() {
    let json = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n";
    let d = repo("fix-json", &[("messages/en.json", json), ("lib/price.ts", PRICE)]);
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "other"]);
    write(&d, "messages/en.json", &json.replace("\"c\": 3", "\"c\": 30"));
    open(&d).save("Theirs").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "messages/en.json", &json.replace("\"a\": 1", "\"a\": 10"));
    open(&d).save("Ours").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "other").unwrap();
    assert!(p.collisions.is_empty());
    assert_eq!(p.text_merged, ["messages/en.json"]);
    let merged = p.files["messages/en.json"].clone().unwrap();
    assert!(merged.contains("\"a\": 10") && merged.contains("\"c\": 30"));
}

#[test]
fn a_function_moved_to_another_file_keeps_the_other_sides_edit() {
    let both = format!("{PRICE}\nexport function discount(n: number) {{\n  return n * 0.9;\n}}\n");
    let d = repo("fix-move", &[("lib/price.ts", &both), ("app/api/buy/route.ts", ROUTE)]);
    open(&d).save("First").unwrap();
    // They move discount into its own file.
    git(&d, &["checkout", "-q", "-b", "split"]);
    write(&d, "lib/price.ts", PRICE);
    write(&d, "lib/discount.ts", "export function discount(n: number) {\n  return n * 0.9;\n}\n");
    open(&d).save("Move discount").unwrap();
    // We change discount where it was.
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/price.ts", &both.replace("n * 0.9", "n * 0.85"));
    open(&d).save("Bigger discount").unwrap();

    let mut v = open(&d);
    let p = merge::plan(&mut v, "split").unwrap();
    assert!(p.collisions.is_empty(), "{:?}", p.collisions.iter().map(|c| &c.name).collect::<Vec<_>>());
    assert_eq!(p.moved, [("discount".to_string(), "lib/price.ts".to_string(), "lib/discount.ts".to_string())]);
    let moved = p.files["lib/discount.ts"].clone().unwrap();
    assert!(moved.contains("n * 0.85"), "{moved}");
    let old = p.files["lib/price.ts"].clone().unwrap();
    assert!(!old.contains("discount"), "{old}");

    // The change report calls it a move, not a deletion plus something new.
    let mut v = open(&d);
    let c = v.commits(5, "split").unwrap().remove(0);
    let r = v.change_of(&c).unwrap();
    let f = r.functions.iter().find(|f| f.sid.ends_with("#discount")).unwrap();
    assert_eq!(f.state, "moved from lib/price.ts");
    assert_eq!(r.functions.len(), 1, "{:?}", r.functions.iter().map(|f| (&f.sid, &f.state)).collect::<Vec<_>>());
}

#[test]
fn methods_are_followed_into_classes_and_out_to_packages() {
    let mut files = BTreeMap::new();
    files.insert(
        "lib/cart.ts".to_string(),
        "export class Cart {\n  total() {\n    return 1;\n  }\n}\n".to_string(),
    );
    files.insert(
        "lib/db.ts".to_string(),
        "import { drizzle } from \"drizzle-orm/libsql\";\nexport const db = drizzle(\"x\");\n".to_string(),
    );
    files.insert(
        "app/api/cart/route.ts".to_string(),
        "import { Cart } from \"../../../lib/cart\";\nimport { db } from \"../../../lib/db\";\n\nfunction show(c: Cart) {\n  return c.total();\n}\n\nexport async function GET() {\n  const cart = new Cart();\n  await db.transaction(async (tx) => tx.insert(1).values(2));\n  const names = [\"a\"].map((x) => x.trim());\n  return Response.json({ t: cart.total(), s: show(cart), names });\n}\n"
            .to_string(),
    );
    let l = flowcontrol::lift_source(&Source::Mem { root: PathBuf::from("/tmp"), files }, "flows").unwrap();
    let f = l.flows.iter().find(|f| f.id == "GET /api/cart").unwrap();
    let names: Vec<String> = f.steps.iter().map(|s| l.repo.slice(s.at).name.clone()).collect();
    assert!(names.contains(&"Cart".to_string()), "{names:?}");
    assert!(f.outside.contains("drizzle-orm"));
    assert_eq!(l.graph.method_calls[3], 0, "unknown method calls: {:?}", l.graph.method_calls);
}

#[test]
fn a_merge_never_destroys_a_binary_file() {
    let d = repo("fix-binary", &[("lib/price.ts", PRICE), ("logo.png", "start")]);
    std::fs::write(d.join("logo.png"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "art"]);
    std::fs::write(d.join("logo.png"), [0u8, 200, 201, 202, 0, 2]).unwrap();
    open(&d).save("New logo").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/price.ts", &PRICE.replace("20", "21"));
    open(&d).save("Price").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "art").unwrap();
    merge::apply(&v, &p, "art").unwrap();
    assert_eq!(std::fs::read(d.join("logo.png")).expect("logo.png was deleted by the merge"), [0u8, 200, 201, 202, 0, 2]);
}

#[test]
fn resolving_a_binary_collision_takes_their_file() {
    let d = repo("fix-binary2", &[("lib/price.ts", PRICE)]);
    std::fs::write(d.join("logo.png"), [0u8, 159, 146, 150]).unwrap();
    open(&d).save("First").unwrap();
    git(&d, &["checkout", "-q", "-b", "art"]);
    std::fs::write(d.join("logo.png"), [0u8, 1, 1, 1]).unwrap();
    open(&d).save("Theirs").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    std::fs::write(d.join("logo.png"), [0u8, 2, 2, 2]).unwrap();
    open(&d).save("Ours").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "art").unwrap();
    assert_eq!(p.collisions.len(), 1);
    merge::apply(&v, &p, "art").unwrap();
    merge::resolve(&v, "logo.png", "theirs").unwrap();
    assert_eq!(std::fs::read(d.join("logo.png")).expect("logo.png was deleted"), [0u8, 1, 1, 1]);
}
