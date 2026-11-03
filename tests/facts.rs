//! The facts layer: what changed, read from the code, with no AI.

use flowcontrol::change::{compact, compare};
use flowcontrol::source::Source;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn lift(files: &[(&str, &str)]) -> flowcontrol::Lift {
    let files: BTreeMap<String, String> = files.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    flowcontrol::lift_source(&Source::Mem { root: PathBuf::from("/tmp"), files }, "flows").unwrap()
}

fn facts(path: &str, a: &str, b: &str) -> Vec<String> {
    let r = compare(&lift(&[(path, a)]), &lift(&[(path, b)]), &Default::default(), vec![]);
    r.functions.iter().flat_map(|f| f.facts.clone()).collect()
}

const PY: &str = "def price(plan):\n    if plan == \"pro\":\n        return 24\n    return charge(0)\n";

#[test]
fn values_conditions_and_calls_are_stated() {
    assert_eq!(facts("a.py", PY, &PY.replace("24", "22")), ["value: 24 -> 22"]);
    assert_eq!(facts("a.py", PY, &PY.replace("plan == \"pro\"", "plan != \"pro\"")), ["condition: plan == \"pro\" -> plan != \"pro\""]);
    assert_eq!(facts("a.py", PY, &PY.replace("charge(0)", "bill(0)")), ["now also calls bill", "no longer calls charge"]);
}

#[test]
fn styling_is_not_content() {
    let a = "export function Hi() {\n  return <div className=\"p-2\">{label(\"x\")}</div>;\n}\n";
    assert_eq!(facts("a.tsx", a, &a.replace("p-2", "p-4")), ["styling changed"]);
}

#[test]
fn new_functions_say_what_they_are() {
    let a = "def one():\n    return 1\n";
    let b = "def one():\n    return 1\n\n\n# Emails the owner.\ndef notify():\n    send(1)\n";
    assert_eq!(facts("a.py", a, b), ["Emails the owner.", "calls send"]);
}

#[test]
fn data_files_report_keys() {
    let a = lift(&[("messages/en.json", "{\"home\":{\"title\":\"Old\",\"cta\":\"Go\"}}")]);
    let b = lift(&[("messages/en.json", "{\"home\":{\"title\":\"New\",\"cta\":\"Go\",\"sub\":\"Hi\"}}")]);
    let r = compare(&a, &b, &Default::default(), vec![]);
    assert_eq!(r.data_facts[0].1, ["home.sub", "home.title"].iter().enumerate().map(|(i, k)| if i == 0 { format!("added {k}: \"Hi\"") } else { format!("{k}: \"Old\" -> \"New\"") }).collect::<Vec<_>>());
    assert!(compact(&r).contains("home.title: \"Old\" -> \"New\""));
}
