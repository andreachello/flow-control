//! The new diff: each journey a change touches, as its steps in order. Changed steps are
//! highlighted with before and after, the rest stay quiet, and the code is one click behind.

use crate::change::{step_label, JState, Report};
use crate::comment::{own_steps, Comment};
use crate::names::Names;
use crate::repo::SRef;
use crate::words::slice_of;
use crate::Lift;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const TEMPLATE: &str = include_str!("map.html");

fn code(l: &Lift, r: SRef) -> String {
    l.repo.slice(r).text(&l.repo.files[r.0])
}

fn step(kind: &str, label: String, sid: &str, facts: Vec<String>, before: Option<String>, after: Option<String>) -> Value {
    json!({ "kind": kind, "label": label, "file": sid.split('#').next().unwrap_or(""), "facts": facts, "before": before, "after": after })
}

/// A journey's steps in order: those it runs now, with those it stopped running at their old place.
fn journey_steps(a: &Lift, b: &Lift, id: &str, title: &str, r: &Report, names: &Names) -> Vec<Value> {
    let (x, y) = (a.flows.iter().position(|f| f.id == id), b.flows.iter().position(|f| f.id == id));
    let was: BTreeMap<&str, &str> = r.functions.iter().filter_map(|f| f.before.as_deref().map(|p| (f.sid.as_str(), p))).collect();
    let facts_of = |sid: &str| r.functions.iter().find(|f| f.sid == sid).map(|f| f.facts.clone()).unwrap_or_default();
    let old: Vec<(String, SRef)> = x.map(|i| own_steps(a, i).map(|s| (a.repo.sid(s.at), s.at)).collect()).unwrap_or_default();
    let new: Vec<(String, SRef)> = y.map(|i| own_steps(b, i).map(|s| (b.repo.sid(s.at), s.at)).collect()).unwrap_or_default();
    let old_at: BTreeMap<&str, SRef> = old.iter().map(|(k, r)| (k.as_str(), *r)).collect();
    let now_ids: BTreeSet<&str> = new.iter().map(|(k, _)| k.as_str()).collect();
    let moved: BTreeSet<&str> = new.iter().filter_map(|(k, _)| was.get(k.as_str()).copied()).collect();

    // (old step id, row), so a step that left can be placed after the one that came before it.
    let mut rows: Vec<(String, Value)> = vec![];
    let mut seen = BTreeSet::new();
    for (sid, r) in &new {
        if !seen.insert(sid.clone()) {
            continue;
        }
        let from = was.get(sid.as_str()).copied().unwrap_or(sid.as_str());
        let label = step_label(names, b, *r);
        let row = match old_at.get(from) {
            Some(o) if a.repo.slice(*o).shape == b.repo.slice(*r).shape && from == sid => step("same", label, sid, vec![], None, Some(code(b, *r))),
            Some(o) => step("changed", label, sid, facts_of(sid), Some(code(a, *o)), Some(code(b, *r))),
            None => step("new", label, sid, facts_of(sid), None, Some(code(b, *r))),
        };
        rows.push((from.to_string(), row));
    }
    let mut prev: Option<String> = None;
    for (sid, r) in &old {
        if !now_ids.contains(sid.as_str()) && !moved.contains(sid.as_str()) {
            let at = prev.as_ref().and_then(|p| rows.iter().position(|(k, _)| k == p)).map(|i| i + 1).unwrap_or(0);
            rows.insert(at, (sid.clone(), step("gone", step_label(names, a, *r), sid, vec![], Some(code(a, *r)), None)));
        }
        prev = Some(sid.clone());
    }
    // A constant the journey reads is not a step, but it is why the journey changed.
    let mut out: Vec<Value> = r
        .functions
        .iter()
        .filter(|f| f.journeys.iter().any(|j| j == title) && !rows.iter().any(|(k, _)| k == &f.sid) && !new.iter().any(|(k, _)| k == &f.sid))
        .filter_map(|f| {
            let after = slice_of(b, &f.sid)?;
            Some(step("changed", f.label.clone(), &f.sid, f.facts.clone(), slice_of(a, &f.sid).map(|o| code(a, o)), Some(code(b, after))))
        })
        .collect();
    out.extend(rows.into_iter().map(|(_, v)| v));
    out
}

/// Only the journeys this change touches, the ones nobody asked for first.
pub fn render(a: &Lift, b: &Lift, r: &Report, c: &Comment, repo: &str, from: &str, to: &str, names: &Names) -> String {
    let entry = |i: &crate::comment::Item, not_asked: bool| {
        json!({
            "title": i.title,
            "says": i.sentence,
            "state": match i.state { JState::New => "new", JState::Gone => "gone", JState::Changed => "changed" },
            "notAsked": not_asked,
            "steps": journey_steps(a, b, &i.id, &i.title, r, names),
        })
    };
    // Every touched journey is drawn, including those one comment line stands for.
    let member = |i: &crate::comment::Item, t: &String| crate::comment::items(r).into_iter().find(|x| &x.title == t).map(|mut x| { x.sentence = i.sentence.clone(); x });
    let side = |list: &[crate::comment::Item], flag: bool| -> Vec<Value> {
        list.iter().flat_map(|i| std::iter::once(entry(i, flag)).chain(i.also.iter().filter_map(|t| member(i, t)).map(|x| entry(&x, flag))).collect::<Vec<_>>()).collect()
    };
    let journeys: Vec<Value> = side(&c.not_asked, true).into_iter().chain(side(&c.changed, false)).collect();
    let data = json!({
        "repo": repo, "from": from, "to": to, "asked": c.asked, "journeys": journeys,
        "untouched": c.untouched, "smaller": c.smaller, "touched": c.touched, "removed": c.removed, "decided": c.decided,
    });
    TEMPLATE.replacen("/*DATA*/null", &data.to_string().replace("</", "<\\/"), 1)
}
