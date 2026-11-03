//! What actually changed, read from the two versions. No AI: the same change always gives the same
//! facts, in any language. Values, conditions, calls and data, stated plainly; reasons stay with
//! the person who wrote the commit message.

use crate::parse::{CallKind, Callee, Role};
use crate::repo::{is_builtin_method, SRef};
use crate::universal::literals;
use crate::Lift;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

fn cut(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s
    }
}

/// What is in `a` and not in `b`, counting repeats, in order.
fn minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut left: BTreeMap<&String, usize> = BTreeMap::new();
    for x in b {
        *left.entry(x).or_default() += 1;
    }
    let mut out = vec![];
    for x in a {
        match left.get_mut(x) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(x.clone()),
        }
    }
    out
}

/// Pair what went with what came when the counts match, otherwise list each.
fn paired(gone: &[String], came: &[String], what: &str) -> Vec<String> {
    let mut out = vec![];
    if gone.len() == came.len() {
        for (x, y) in gone.iter().zip(came) {
            out.push(format!("{what}: {} -> {}", cut(x, 44), cut(y, 44)));
        }
    } else {
        for y in came {
            out.push(format!("added {what}: {}", cut(y, 60)));
        }
        for x in gone {
            out.push(format!("removed {what}: {}", cut(x, 60)));
        }
    }
    out
}

/// The conditions a block tests, one per line, in any language.
fn conditions(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*(?:\}\s*)?(?:else\s+)?(if|elif|elsif|unless|while|when|guard)\b(.*)$").unwrap());
    text.lines()
        .filter_map(|l| re.captures(l).map(|c| c[2].trim().trim_end_matches(['{', ':']).trim_end_matches(" then").trim_end_matches(" do").trim().to_string()))
        .filter(|c| !c.is_empty())
        .collect()
}

fn called(l: &Lift, r: SRef) -> BTreeSet<String> {
    l.repo
        .slice(r)
        .calls
        .iter()
        .filter(|c| c.kind != CallKind::Ref)
        .filter_map(|c| match &c.callee {
            Callee::Ident(n) => Some(n.clone()),
            Callee::Member(o, p) if !is_builtin_method(p) => Some(format!("{o}.{p}")),
            Callee::Method(_, p) if !is_builtin_method(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn writes(l: &Lift, r: SRef) -> BTreeSet<String> {
    l.graph.writes.get(&r).map(|w| w.iter().map(|k| k.rsplit('#').next().unwrap_or(k).trim_start_matches("S.").to_string()).collect()).unwrap_or_default()
}

fn names(v: &[String]) -> String {
    if v.len() <= 4 {
        v.join(", ")
    } else {
        format!("{} and {} more", v[..4].join(", "), v.len() - 4)
    }
}

/// What differs between two versions of one function or value.
pub fn edited(a: &Lift, ra: SRef, b: &Lift, rb: SRef) -> Vec<String> {
    if b.repo.slice(rb).role == Role::Type {
        return vec!["the shape of this data changed".into()];
    }
    let (fa, fb) = (&a.repo.files[ra.0], &b.repo.files[rb.0]);
    let (ta, tb) = (a.repo.slice(ra).text(fa), b.repo.slice(rb).text(fb));
    let mut out = vec![];

    let (wa, wb) = (writes(a, ra), writes(b, rb));
    let now: Vec<String> = wb.difference(&wa).cloned().collect();
    let stop: Vec<String> = wa.difference(&wb).cloned().collect();
    if !now.is_empty() {
        out.push(format!("now changes {}", names(&now)));
    }
    if !stop.is_empty() {
        out.push(format!("no longer changes {}", names(&stop)));
    }

    let (la, lb) = (literals(&fa.path, &ta), literals(&fb.path, &tb));
    let vals = |v: &[(String, bool)]| -> Vec<String> { v.iter().filter(|x| !x.1).map(|x| x.0.clone()).collect() };
    let (va, vb) = (vals(&la), vals(&lb));
    out.extend(paired(&minus(&va, &vb), &minus(&vb, &va), "value"));

    let (ca, cb) = (conditions(&ta), conditions(&tb));
    out.extend(paired(&minus(&ca, &cb), &minus(&cb, &ca), "condition"));

    let (ka, kb) = (called(a, ra), called(b, rb));
    let more: Vec<String> = kb.difference(&ka).cloned().collect();
    let fewer: Vec<String> = ka.difference(&kb).cloned().collect();
    if !more.is_empty() {
        out.push(format!("now also calls {}", names(&more)));
    }
    if !fewer.is_empty() {
        out.push(format!("no longer calls {}", names(&fewer)));
    }

    let style = |v: &[(String, bool)]| -> Vec<String> { v.iter().filter(|x| x.1).map(|x| x.0.clone()).collect() };
    if style(&la) != style(&lb) {
        out.push("styling changed".into());
    }
    if out.is_empty() {
        // No value, condition or call moved, so the difference is in which names it uses.
        let uniq = |v: Vec<String>| v.into_iter().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        let (wa, wb) = (crate::universal::words(&fa.path, &ta), crate::universal::words(&fb.path, &tb));
        let (more, fewer) = (uniq(minus(&wb, &wa)), uniq(minus(&wa, &wb)));
        if !more.is_empty() {
            out.push(format!("now uses {}", names(&more)));
        }
        if !fewer.is_empty() {
            out.push(format!("no longer uses {}", names(&fewer)));
        }
    }
    if out.is_empty() {
        out.push("the code was reordered, with the same contents".into());
    }
    out
}

/// What a new function says about itself and does.
pub fn added(l: &Lift, r: SRef) -> Vec<String> {
    let f = &l.repo.files[r.0];
    let mut out = vec![];
    let doc = crate::view::doc(l.repo.slice(r), f);
    if !doc.is_empty() {
        out.push(cut(&doc, 90));
    }
    let w: Vec<String> = writes(l, r).into_iter().collect();
    if !w.is_empty() {
        out.push(format!("changes {}", names(&w)));
    }
    let c: Vec<String> = called(l, r).into_iter().collect();
    if !c.is_empty() {
        out.push(format!("calls {}", names(&c)));
    }
    out
}

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut BTreeMap<String, String>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                flatten(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, x, out);
            }
        }
        x => {
            out.insert(prefix.to_string(), x.to_string());
        }
    }
}

fn keys_of(path: &str, text: &str) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if path.ends_with(".json") {
        flatten("", &serde_json::from_str(text).ok()?, &mut out);
        return Some(out);
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*-?\s*([\w.\-]+)\s*[:=]\s*(.+?)\s*$").unwrap());
    if [".yaml", ".yml", ".toml", ".ini", ".env"].iter().any(|e| path.ends_with(e)) {
        for l in text.lines() {
            if let Some(c) = re.captures(l).filter(|_| !l.trim_start().starts_with('#')) {
                out.insert(c[1].to_string(), c[2].to_string());
            }
        }
        return Some(out);
    }
    None
}

/// Which settings or strings changed in a data file. Empty for formats with no keys.
pub fn data_file(path: &str, before: &str, after: &str) -> Vec<String> {
    let (Some(a), Some(b)) = (keys_of(path, before), keys_of(path, after)) else {
        if path.ends_with(".md") || path.ends_with(".mdx") {
            let heads = |t: &str| t.lines().filter(|l| l.starts_with('#')).map(|l| l.trim_start_matches('#').trim().to_string()).collect::<Vec<_>>();
            let (x, y) = (heads(before), heads(after));
            return paired(&minus(&x, &y), &minus(&y, &x), "heading");
        }
        return vec![];
    };
    let mut out = vec![];
    for (k, v) in &b {
        match a.get(k) {
            Some(o) if o != v => out.push(format!("{k}: {} -> {}", cut(o, 40), cut(v, 40))),
            None => out.push(format!("added {k}: {}", cut(v, 50))),
            _ => {}
        }
    }
    for k in a.keys().filter(|k| !b.contains_key(*k)) {
        out.push(format!("removed {k}"));
    }
    out
}
