//! Flow without a grammar: Python, Go, Java and a language nobody has heard of.

use flowcontrol::source::Source;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn lift(files: &[(&str, &str)]) -> flowcontrol::Lift {
    let files: BTreeMap<String, String> = files.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    flowcontrol::lift_source(&Source::Mem { root: PathBuf::from("/tmp"), files }, "flows").unwrap()
}

fn steps(l: &flowcontrol::Lift, id: &str) -> Vec<String> {
    let f = l.flows.iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no journey {id}; have {:?}", l.flows.iter().map(|f| &f.id).collect::<Vec<_>>()));
    f.steps.iter().map(|s| l.repo.slice(s.at).name.clone()).collect()
}

fn exact(l: &flowcontrol::Lift) {
    for f in &l.repo.files {
        let led = f.ledger();
        assert!(led.complete && led.round_trip, "{} does not round trip", f.path);
    }
}

const PY_MAIN: &str = r#"from fastapi import FastAPI
import billing

app = FastAPI()


# A person upgrades their plan.
@app.post("/upgrade")
async def upgrade(plan: str):
    price = billing.price_for(plan)
    billing.charge(price)
    return {"ok": True}


@app.get("/health")
def health():
    return "ok"
"#;

const PY_BILLING: &str = r#"TAX = 0.2


def price_for(plan):
    base = 24 if plan == "pro" else 0
    return with_tax(base)


def with_tax(amount):
    return amount * (1 + TAX)


def charge(amount):
    print("charged", amount)
"#;

#[test]
fn python_routes_become_journeys() {
    let l = lift(&[("app/main.py", PY_MAIN), ("app/billing.py", PY_BILLING)]);
    exact(&l);
    assert_eq!(steps(&l, "POST /upgrade"), ["upgrade", "price_for", "with_tax", "charge"]);
    assert_eq!(steps(&l, "GET /health"), ["health"]);
}

#[test]
fn go_handlers_and_main() {
    let main = "package main\n\nimport \"net/http\"\n\nfunc main() {\n\thttp.HandleFunc(\"/hello\", hello)\n\thttp.ListenAndServe(\":8080\", nil)\n}\n\nfunc hello(w http.ResponseWriter, r *http.Request) {\n\tw.Write([]byte(greet(\"you\")))\n}\n";
    let util = "package main\n\n// greet builds the greeting.\nfunc greet(name string) string {\n\treturn \"hi \" + name\n}\n";
    let l = lift(&[("main.go", main), ("util.go", util)]);
    exact(&l);
    assert_eq!(steps(&l, "ROUTE /hello"), ["hello", "greet"]);
    assert!(l.flows.iter().any(|f| f.kind == "program"));
}

#[test]
fn java_spring_controller() {
    let java = "package app;\n\nimport org.springframework.web.bind.annotation.*;\n\n@RestController\npublic class Users {\n    @GetMapping(\"/users\")\n    public List<User> list() {\n        return repo.findAll();\n    }\n}\n";
    let l = lift(&[("src/Users.java", java)]);
    exact(&l);
    assert!(l.flows.iter().any(|f| f.id == "GET /users"), "{:?}", l.flows.iter().map(|f| &f.id).collect::<Vec<_>>());
}

#[test]
fn a_language_nobody_has_heard_of() {
    let zz = "~~ the greeter, in a made-up language\ndefine greet(name) [\n  say(\"hi \" ++ name)\n]\n\ndefine welcome(name) [\n  greet(name)\n  log(name)\n]\n";
    let toml = "[[entry]]\nfile = \"hello.zz\"\nexport = \"welcome\"\nid = \"Someone is welcomed\"\n";
    let l = lift(&[("hello.zz", zz), ("flow.toml", toml)]);
    exact(&l);
    assert_eq!(steps(&l, "Someone is welcomed"), ["welcome", "greet"]);
}

#[test]
fn python_functions_merge_one_at_a_time() {
    let ours = PY_BILLING.replace("24 if", "29 if");
    let theirs = PY_BILLING.replace("print(\"charged\", amount)", "print(\"charged\", round(amount, 2))");
    let m = flowcontrol::merge::merge_file("app/billing.py", Some(PY_BILLING), &ours, &theirs).unwrap();
    assert!(m.collisions.is_empty());
    let t = m.text.unwrap();
    assert!(t.contains("29 if") && t.contains("round(amount, 2)"), "{t}");
    // Same function, same line, two ways: a collision.
    let rival = PY_BILLING.replace("24 if", "19 if");
    let m = flowcontrol::merge::merge_file("app/billing.py", Some(PY_BILLING), &ours, &rival).unwrap();
    assert_eq!(m.collisions.len(), 1);
    assert_eq!(m.collisions[0].name, "price_for");
}

#[test]
fn status_on_a_python_repo_speaks_in_journeys() {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("uni-status");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/main.py"), PY_MAIN).unwrap();
    std::fs::write(d.join("app/billing.py"), PY_BILLING).unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&d).args(args).output().unwrap().status.success());
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    let open = || flowcontrol::vcs::Vcs::open(&d, "flows", Default::default()).unwrap();
    open().save("First").unwrap();
    std::fs::write(d.join("app/billing.py"), PY_BILLING.replace("TAX = 0.2", "TAX = 0.22").replace("return amount * (1 + TAX)", "return  amount*(1+TAX)")).unwrap();
    let r = open().status().unwrap();
    let ids: Vec<&str> = r.journeys.iter().map(|j| j.id.as_str()).collect();
    // TAX changed (information the journey reads); with_tax was only reformatted.
    assert!(r.functions.iter().any(|f| f.sid.ends_with("#with_tax") && f.state == "reformatted only"), "{:?}", r.functions.iter().map(|f| (&f.sid, &f.state)).collect::<Vec<_>>());
    assert!(r.functions.iter().any(|f| f.sid.ends_with("#TAX") && f.state == "edited"));
    let _ = ids;
}

#[test]
fn class_methods_are_their_own_parts() {
    let py = "class Cart:\n    rate = 2\n\n    @app.post(\"/total\")\n    def total(self):\n        return self.sum() * rate\n\n    def sum(self):\n        return 1\n";
    let l = lift(&[("cart.py", py)]);
    exact(&l);
    assert_eq!(steps(&l, "POST /total"), ["Cart.total", "Cart.sum"]);

    let java = "public class Users {\n    private int n;\n\n    @GetMapping(\"/users\")\n    public List<User> list() {\n        return load();\n    }\n\n    private List<User> load() {\n        return repo.findAll();\n    }\n}\n";
    let l = lift(&[("src/Users.java", java)]);
    exact(&l);
    assert_eq!(steps(&l, "GET /users"), ["Users.list", "Users.load"]);
}

#[test]
fn merging_inside_a_class_keeps_the_file_in_order() {
    let base = "public class A {\n    int n;\n\n    int one() {\n        return 1;\n    }\n\n    int two() {\n        return 2;\n    }\n}\n";
    let ours = base.replace("return 1;", "return 10;");
    let theirs = base.replace("return 2;", "return 20;").replace("int n;", "int n = 5;");
    let m = flowcontrol::merge::merge_file("src/A.java", Some(base), &ours, &theirs).unwrap();
    assert!(m.collisions.is_empty(), "{:?}", m.collisions);
    assert_eq!(m.text.unwrap(), "public class A {\n    int n = 5;\n\n    int one() {\n        return 10;\n    }\n\n    int two() {\n        return 20;\n    }\n}\n");
}

#[test]
fn a_repo_with_no_entry_points_starts_from_what_nothing_calls() {
    let lib = "def load(path):\n    return parse(read(path))\n\n\ndef parse(text):\n    return text\n\n\ndef read(path):\n    return open(path)\n";
    let l = lift(&[("lib/io.py", lib)]);
    assert_eq!(l.flows.len(), 1);
    assert_eq!(l.flows[0].kind, "start");
    assert_eq!(steps(&l, "start load"), ["load", "read", "parse"]);
}

#[test]
fn text_inside_strings_is_not_code() {
    let rs = "const SAMPLE: &str = r#\"\n@app.post(\"/fake\")\ndef nope():\n    pass\n\"#;\n\nfn main() {\n    run();\n}\n\nfn run() {}\n";
    let l = lift(&[("src/main.rs", rs)]);
    exact(&l);
    assert!(!l.flows.iter().any(|f| f.id.contains("/fake")));
    assert_eq!(steps(&l, "program src"), ["main", "run"]);
}

#[test]
fn a_local_variable_does_not_link_to_a_namesake() {
    let a = "def report(items):\n    summary = len(items)\n    show(summary)\n\n\ndef show(x):\n    print(x)\n";
    let b = "def summary():\n    return 1\n";
    let l = lift(&[("a.py", a), ("b.py", b), ("main.py", "if __name__ == \"__main__\":\n    report([])\n")]);
    let s = steps(&l, "script main.py");
    assert!(s.contains(&"show".to_string()) && !s.contains(&"summary".to_string()), "{s:?}");
}

#[test]
fn flat_code_splits_at_paragraphs_labels_and_keywords() {
    let mut asm = String::from("start:\nmov a, 1\nmov b, 2\nadd a, b\n\n");
    for i in 0..12 {
        asm += &format!("loop{i}:\ncmp a, {i}\njne loop{i}\nret\n\n");
    }
    let l = lift(&[("boot.s", &asm)]);
    exact(&l);
    let f = &l.repo.files[0];
    assert_eq!(f.slices.len() - 1, 13, "{:?}", f.slices.iter().map(|s| &s.name).collect::<Vec<_>>());
    assert_eq!(f.slices[f.by_name["start"]].lines.len(), 4);

    let mut lua = String::new();
    for i in 0..8 {
        lua += &format!("function f{i}(x)\nlocal y = x\nprint(y)\nreturn y\nend\n");
    }
    let l = lift(&[("a.lua", &lua)]);
    exact(&l);
    assert_eq!(l.repo.files[0].slices.len() - 1, 8, "{:?}", l.repo.files[0].slices.iter().map(|s| &s.name).collect::<Vec<_>>());
}

#[test]
fn generated_files_are_one_unit() {
    let min = format!("!function(){{{}}}();\n", "var a=1;".repeat(1000));
    let l = lift(&[("public/app.min.js", &min), ("gen.go", "// Code generated by protoc. DO NOT EDIT.\npackage x\n\nfunc A() {}\n")]);
    exact(&l);
    for f in &l.repo.files {
        assert_eq!(f.slices.len(), 2, "{}", f.path);
    }
}

#[test]
fn deep_nesting_reaches_every_method() {
    let rb = "module Billing\n  class Cart\n    def total\n      sum\n    end\n\n    def sum\n      1\n    end\n  end\nend\n";
    let l = lift(&[("cart.rb", rb)]);
    exact(&l);
    let f = &l.repo.files[0];
    assert!(f.by_name.contains_key("Billing.Cart.total"), "{:?}", f.slices.iter().map(|s| &s.name).collect::<Vec<_>>());
    let cs = "namespace App.Web {\n    public class Users {\n        [HttpGet(\"/users\")]\n        public string List() {\n            return Load();\n        }\n\n        private string Load() {\n            return \"x\";\n        }\n    }\n}\n";
    let l = lift(&[("Users.cs", cs)]);
    exact(&l);
    assert!(l.repo.files[0].by_name.keys().any(|k| k.ends_with("Users.List")), "{:?}", l.repo.files[0].by_name.keys().collect::<Vec<_>>());
}

#[test]
fn imports_decide_between_namesakes() {
    let a = "def save():\n    return 1\n";
    let b = "def save():\n    return 2\n";
    let main = "from pkg.a import save\n\nif __name__ == \"__main__\":\n    save()\n";
    let l = lift(&[("pkg/a.py", a), ("pkg/b.py", b), ("main.py", main)]);
    let f = l.flows.iter().find(|f| f.id == "script main.py").unwrap();
    let sids: Vec<String> = f.steps.iter().map(|s| l.repo.sid(s.at)).collect();
    assert!(sids.contains(&"pkg/a.py#save".to_string()) && !sids.contains(&"pkg/b.py#save".to_string()), "{sids:?}");
}

#[test]
fn a_typescript_class_is_split_into_methods() {
    let ts = "export class Cart {\n  total() {\n    return this.sum();\n  }\n\n  sum() {\n    return 1;\n  }\n\n  unused() {\n    return 2;\n  }\n}\n";
    let route = "import { Cart } from \"../../../lib/cart\";\n\nexport async function GET() {\n  const c = new Cart();\n  return Response.json({ t: c.total() });\n}\n";
    let l = lift(&[("lib/cart.ts", ts), ("app/api/cart/route.ts", route)]);
    exact(&l);
    let names = steps(&l, "GET /api/cart");
    assert!(names.contains(&"Cart.total".to_string()) && names.contains(&"Cart.sum".to_string()) && !names.contains(&"Cart.unused".to_string()), "{names:?}");
}

#[test]
fn each_command_of_a_cli_is_a_journey() {
    let rs = "fn main() {\n    run();\n}\n\nfn run() {\n    match cli.cmd {\n        Cmd::Status { full } => {\n            status();\n        }\n        Cmd::Merge { branch } => {\n            merge(branch);\n            log();\n        }\n        Cmd::Log => log(),\n    }\n}\n\nfn status() {}\n\nfn merge(b: String) {}\n\nfn log() {}\n";
    let l = lift(&[("src/main.rs", rs)]);
    let ids: Vec<&str> = l.flows.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["command log", "command merge", "command status"]);
    assert_eq!(steps(&l, "command merge"), ["run", "merge", "log"]);
}
