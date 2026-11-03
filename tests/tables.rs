//! `x[name]()` on a table written in the code: flow reads the table instead of giving up.

use std::path::{Path, PathBuf};

fn repo(name: &str, plugins: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tables").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("app/api/run")).unwrap();
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::write(d.join("app/api/run/route.ts"), "import { run } from \"@/lib/plugins\";\nexport async function GET(req: Request) {\n  return Response.json(run(new URL(req.url).searchParams.get(\"p\")!));\n}\n").unwrap();
    std::fs::write(d.join("tsconfig.json"), "{\"compilerOptions\":{\"paths\":{\"@/*\":[\"./*\"]}}}").unwrap();
    std::fs::write(d.join("lib/plugins.ts"), plugins).unwrap();
    d
}

fn lift(d: &Path) -> flowcontrol::Lift {
    flowcontrol::lift(d, "flows").unwrap()
}

fn steps(l: &flowcontrol::Lift) -> Vec<String> {
    l.flows[0].steps.iter().map(|s| l.repo.slice(s.at).name.clone()).collect()
}

const RUN: &str = "export function run(name: string) {\n  return plugins[name]();\n}\n";

#[test]
fn a_fixed_table_joins_the_journey_with_the_functions_it_names() {
    let d = repo("fixed", &format!("function summarize() {{\n  return \"s\";\n}}\nconst plugins = {{\n  summarize,\n  translate: () => \"t\",\n}};\n{RUN}"));
    let l = lift(&d);
    assert_eq!(steps(&l), ["GET", "run", "plugins", "summarize"]);
    assert!(l.flows[0].unresolved.is_empty());
}

#[test]
fn a_table_that_can_grow_keeps_its_warning() {
    for (name, table) in [
        ("spread", "const more = {};\nconst plugins = { ...more, a: () => 1 };\n"),
        ("let", "let plugins = { a: () => 1 };\n"),
        ("computed", "const k = \"a\";\nconst plugins = { [k]: () => 1 };\n"),
    ] {
        let l = lift(&repo(name, &format!("{table}{RUN}")));
        assert!(steps(&l).contains(&"plugins".to_string()), "{name}");
        assert_eq!(l.flows[0].unresolved.len(), 1, "{name}");
        assert!(l.flows[0].unresolved[0].1.why.contains("can change"), "{name}");
    }
}

#[test]
fn a_table_written_elsewhere_keeps_its_warning() {
    let src = format!("const plugins = {{ a: () => 1 }};\nexport function add() {{\n  plugins.extra = () => 2;\n}}\n{RUN}");
    let l = lift(&repo("written", &src));
    assert_eq!(l.flows[0].unresolved.len(), 1);
}

#[test]
fn a_lookup_on_something_that_is_not_a_table_is_still_reported() {
    let l = lift(&repo("param", "export function run(table: any, name: string) {\n  return table[name]();\n}\n"));
    assert_eq!(l.flows[0].unresolved.len(), 1);
    assert!(l.flows[0].unresolved[0].1.why.contains("looked up by name"));
}

#[test]
fn adding_a_plugin_now_changes_the_journey_that_can_run_it() {
    let table = |extra: &str| format!("const plugins = {{\n  a: () => 1,\n{extra}}};\n{RUN}");
    let before = lift(&repo("add-before", &table("")));
    let after = lift(&repo("add-after", &table("  b: () => 2,\n")));
    let r = flowcontrol::affected::affected(&before, &after);
    assert_eq!(r.flows.len(), 1);
    assert_eq!(r.flows[0].edited, ["lib/plugins.ts#plugins"]);
    assert!(r.loose.is_empty());
}
