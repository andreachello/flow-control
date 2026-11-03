//! `flow view`: one self-contained HTML page built from a lift, for looking around.

use crate::util::ranges;
use crate::Lift;
use serde_json::{json, Map, Value};

const TEMPLATE: &str = include_str!("view.html");

/// The first sentence of the comment written above a declaration, if anyone wrote one.
fn doc(sl: &crate::parse::Slice, f: &crate::parse::ParsedFile) -> String {
    let mut words = vec![];
    for &n in &sl.lines {
        let t = f.lines[n].trim();
        if t.is_empty() && words.is_empty() {
            continue;
        }
        let is_comment = t.starts_with("//") || t.starts_with("/*") || t.starts_with('*');
        if !is_comment {
            break;
        }
        let body = t.trim_start_matches("/**").trim_start_matches("/*").trim_start_matches("//").trim_start_matches('*').trim_end_matches("*/").trim();
        if body.starts_with('@') || body.starts_with("eslint") || body.starts_with("biome-ignore") || body.starts_with("ts-") {
            continue;
        }
        if !body.is_empty() {
            words.push(body.to_string());
        }
    }
    let text = words.join(" ");
    let end = text.find(". ").map(|i| i + 1).unwrap_or(text.len());
    let first: String = text[..end].chars().take(220).collect();
    first
}

/// The last saved changes, told in journeys, for the Changes timeline. Empty outside a git repo.
pub fn history(root: &std::path::Path, out: &str, names: &crate::names::Names, n: usize) -> Value {
    let Ok(mut v) = crate::vcs::Vcs::open(root, out, names.clone()) else { return json!([]) };
    if v.head().is_none() {
        return json!([]);
    }
    let Ok(commits) = v.commits_lifted(n, "HEAD") else { return json!([]) };
    let mut out = vec![];
    for c in commits {
        let Ok(r) = v.change_of(&c) else { continue };
        out.push(json!({
            "id": &c.id[..c.id.len().min(8)],
            "date": c.date,
            "subject": c.subject,
            "merge": c.parents.len() > 1,
            "summary": crate::change::one_line(&r),
            "untouched": r.untouched,
            "journeys": r.journeys.iter().map(|j| json!({
                "id": j.id, "title": j.title,
                "state": match j.state { crate::change::JState::New => "new", crate::change::JState::Gone => "gone", crate::change::JState::Changed => "changed" },
                "edited": j.edited, "added": j.added, "removed": j.removed, "nowChanges": j.now_changes,
            })).collect::<Vec<_>>(),
            "code": r.functions.iter().filter(|f| f.journeys.is_empty()).map(|f| f.label.clone()).collect::<Vec<_>>(),
            "other": r.other_files,
        }));
    }
    Value::Array(out)
}

pub fn render(l: &Lift, repo_name: &str, names: &crate::names::Names) -> String {
    render_with_history(l, repo_name, names, json!([]))
}

pub fn render_with_history(l: &Lift, repo_name: &str, names: &crate::names::Names, history: Value) -> String {
    let repo = &l.repo;
    let flows: Vec<Value> = l
        .flows
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            let steps: Vec<Value> = f
                .steps
                .iter()
                .map(|s| {
                    let sl = repo.slice(s.at);
                    json!({
                        "sid": repo.sid(s.at),
                        "name": sl.name,
                        "file": repo.files[s.at.0].path,
                        "lines": ranges(&sl.lines),
                        "depth": s.depth,
                        "via": s.via,
                        "role": sl.role.label(),
                        "doc": doc(sl, &repo.files[s.at.0]),
                        "label": names.steps.get(&repo.sid(s.at)).map(|n| n.name.clone()),
                        "labelStale": names.steps.get(&repo.sid(s.at)).map(|n| n.hash != crate::output::hash_of(repo, s.at)).unwrap_or(false),
                        "reads": l.graph.reads.get(&s.at).map(|r| r.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                        "changes": l.graph.writes.get(&s.at).map(|r| r.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                    })
                })
                .collect();
            let jn = names.journeys.get(&f.id);
            json!({
                "title": jn.map(|n| n.title.clone()),
                "summary": jn.map(|n| n.summary.clone()),
                "area": jn.map(|n| n.area.clone()),
                "nameBy": jn.map(|n| n.by.clone()),
                "nameStale": jn.map(|n| n.hash != crate::names::journey_hash(l, fi)).unwrap_or(false),
                "scenario": repo.cfg.scenario.iter().find(|s| s.journey.as_deref() == Some(f.id.as_str())).map(|s| s.name.clone()),
                "id": f.id,
                "slug": f.slug,
                "kind": f.kind,
                "entry": repo.sid(f.entry),
                "steps": steps,
                "outside": f.outside.iter().collect::<Vec<_>>(),
                "unresolved": f.unresolved.iter().map(|(at, u)| json!({
                    "at": format!("{}:{}", repo.files[at.0].path, u.line + 1), "code": u.code, "why": u.why
                })).collect::<Vec<_>>(),
                "notFollowed": f.not_followed,
            })
        })
        .collect();

    // Source text for every slice a journey or a fact points at.
    let mut slices = Map::new();
    let mut want: Vec<(usize, usize)> = l.flows.iter().flat_map(|f| f.steps.iter().map(|s| s.at)).collect();
    for (fi, f) in repo.files.iter().enumerate() {
        for si in 0..f.slices.len() {
            if l.out.stats.loose.contains(&repo.sid((fi, si))) {
                want.push((fi, si));
            }
        }
    }
    for r in want {
        let id = repo.sid(r);
        if slices.contains_key(&id) {
            continue;
        }
        let f = &repo.files[r.0];
        let lines: Vec<Value> = repo.slice(r).lines.iter().map(|&n| json!([n + 1, f.lines[n]])).collect();
        slices.insert(id, Value::Array(lines));
    }

    let s = &l.out.stats;
    let at = |v: &Vec<(String, usize, String, String)>| -> Vec<Value> {
        v.iter().map(|(f, line, code, why)| json!({"at": format!("{f}:{line}"), "code": code, "why": why})).collect()
    };
    // Every file, every line and its owner: the ledger, so the file tree can show how code maps to journeys.
    let mut used: std::collections::HashMap<(usize, usize), Vec<usize>> = Default::default();
    for (i, f) in l.flows.iter().enumerate() {
        for st in &f.steps {
            let v = used.entry(st.at).or_default();
            if !v.contains(&i) {
                v.push(i);
            }
        }
    }
    let loose: std::collections::HashSet<&String> = l.out.stats.loose.iter().collect();
    let files: Vec<Value> = repo
        .files
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            let owner: Vec<i64> = f.owner.iter().map(|o| match o { crate::parse::Owner::Slice(i) => *i as i64, crate::parse::Owner::Unplaced(_) => -1 }).collect();
            let sl: Vec<Value> = f
                .slices
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let r = (fi, si);
                    let sid = repo.sid(r);
                    json!({
                        "name": s.name,
                        "label": names.steps.get(&sid).map(|n| n.name.clone()),
                        "role": s.role.label(),
                        "lines": ranges(&s.lines),
                        "doc": doc(s, f),
                        "journeys": used.get(&r).cloned().unwrap_or_default(),
                        "reads": l.graph.reads.get(&r).map(|x| x.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                        "changes": l.graph.writes.get(&r).map(|x| x.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                        "loose": loose.contains(&sid),
                        "onLoad": l.graph.on_load.contains(&r),
                        "hand": s.hand.len(),
                    })
                })
                .collect();
            json!({
                "path": f.path,
                "tooBig": f.src.len() > 1_500_000,
                "lines": if f.src.len() > 1_500_000 { Vec::new() } else { f.lines.clone() },
                "owner": if f.src.len() > 1_500_000 { Vec::new() } else { owner },
                "slices": sl,
                "unplaced": f.unplaced.iter().map(|u| json!({"line": u.first_line + 1, "code": u.text, "why": u.why})).collect::<Vec<_>>(),
                "parseErrors": f.error_lines.iter().map(|x| x + 1).collect::<Vec<_>>(),
            })
        })
        .collect();

    let data = json!({
        "files": files,
        "repo": repo_name,
        "history": history,
        "fingerprint": l.out.fingerprint,
        "flows": flows,
        "slices": slices,
        "stats": {
            "files": s.files, "lines": s.lines, "slices": s.slices,
            "unplaced": at(&s.unplaced), "unresolved": at(&s.unresolved),
            "loose": s.loose, "parseErrors": s.parse_errors,
        },
    });
    // Keep "</script>" inside strings from closing the tag.
    let json = data.to_string().replace("</", "<\\/");
    TEMPLATE.replacen("/*DATA*/null", &json, 1)
}
