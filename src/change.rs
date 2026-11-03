//! What changed between two snapshots, said in journeys and functions. No lines, no diffs.

use crate::names::Names;
use crate::output::hash_of;
use crate::parse::SUPPORT;
use crate::util::human;
use crate::Lift;
use std::collections::{BTreeMap, BTreeSet};

pub fn journey_title(names: &Names, id: &str) -> String {
    names.journeys.get(id).map(|n| n.title.clone()).unwrap_or_else(|| id.to_string())
}

fn step_label(names: &Names, l: &Lift, r: (usize, usize)) -> String {
    names.steps.get(&l.repo.sid(r)).map(|n| n.name.clone()).unwrap_or_else(|| human(&l.repo.slice(r).name))
}

fn key_label(k: &str) -> String {
    human(k.rsplit('#').next().unwrap_or(k).trim_start_matches("S.")).to_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JState {
    New,
    Gone,
    Changed,
}

#[derive(Debug, Clone)]
pub struct JourneyChange {
    pub id: String,
    pub title: String,
    pub state: JState,
    pub edited: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub now_changes: Vec<String>,
    pub no_longer_changes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FnChange {
    pub sid: String,
    pub label: String,
    pub state: String,
    pub journeys: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub journeys: Vec<JourneyChange>,
    pub untouched: usize,
    pub functions: Vec<FnChange>,
    pub setup_changed: Vec<String>,
    pub new_unplaced: Vec<String>,
    pub other_files: Vec<String>,
    pub packages: Vec<PackageChange>,
}

#[derive(Debug, Clone)]
pub struct PackageChange {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub journeys: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.journeys.is_empty() && self.functions.is_empty() && self.setup_changed.is_empty() && self.other_files.is_empty() && self.new_unplaced.is_empty() && self.packages.is_empty()
    }
}

/// sid -> (slice, content hash) for every slice that holds code.
/// sid -> (slice, text hash, behaviour fingerprint).
fn slices(l: &Lift) -> BTreeMap<String, ((usize, usize), String, String)> {
    let mut m = BTreeMap::new();
    for (fi, f) in l.repo.files.iter().enumerate() {
        for si in 0..f.slices.len() {
            if si != SUPPORT && !f.slices[si].lines.is_empty() {
                m.insert(l.repo.sid((fi, si)), ((fi, si), hash_of(&l.repo, (fi, si)), f.slices[si].shape.clone()));
            }
        }
    }
    m
}

/// A journey's steps by behaviour: reformatting a step does not change the journey. Steps that
/// moved to another file are keyed by where they live now.
fn steps(l: &Lift, fi: usize, moved: &BTreeMap<String, String>) -> BTreeMap<String, ((usize, usize), String)> {
    l.flows[fi]
        .steps
        .iter()
        .map(|s| {
            let sid = l.repo.sid(s.at);
            (moved.get(&sid).cloned().unwrap_or(sid), (s.at, l.repo.slice(s.at).shape.clone()))
        })
        .collect()
}

fn writes(l: &Lift, fi: usize) -> BTreeSet<String> {
    l.flows[fi].steps.iter().flat_map(|s| l.graph.writes.get(&s.at).cloned().unwrap_or_default()).collect()
}

fn journeys_using(l: &Lift, names: &Names, r: (usize, usize)) -> Vec<String> {
    l.flows.iter().filter(|f| f.steps.iter().any(|s| s.at == r)).map(|f| journey_title(names, &f.id)).collect()
}

/// Compare snapshot `a` (before) with `b` (after).
pub fn compare(a: &Lift, b: &Lift, names: &Names, other_files: Vec<String>) -> Report {
    let mut rep = Report { other_files, ..Default::default() };
    let ai: BTreeMap<&str, usize> = a.flows.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    let bi: BTreeMap<&str, usize> = b.flows.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    let ids: BTreeSet<&str> = ai.keys().chain(bi.keys()).copied().collect();

    // Functions that moved or were renamed: gone in one place, new in another, same name in another
    // file or the same behaviour under another name.
    let (la, lb) = (slices(a), slices(b));
    let gone: Vec<&String> = la.keys().filter(|k| !lb.contains_key(*k)).collect();
    let new: Vec<&String> = lb.keys().filter(|k| !la.contains_key(*k)).collect();
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let name = |sid: &str| sid.rsplit('#').next().unwrap_or("").to_string();
    for g in &gone {
        let (_, _, gs) = &la[*g];
        let hit = new
            .iter()
            .find(|n| !moved.values().any(|v| v == **n) && name(n) == name(g) && new.iter().filter(|m| name(m) == name(g)).count() == 1)
            .or_else(|| new.iter().find(|n| !moved.values().any(|v| v == **n) && &lb[**n].2 == gs && !gs.is_empty()));
        if let Some(n) = hit {
            moved.insert((*g).clone(), (*n).clone());
        }
    }
    for id in ids {
        let title = journey_title(names, id);
        match (ai.get(id), bi.get(id)) {
            (None, Some(_)) => rep.journeys.push(JourneyChange { id: id.into(), title, state: JState::New, edited: vec![], added: vec![], removed: vec![], now_changes: vec![], no_longer_changes: vec![] }),
            (Some(_), None) => rep.journeys.push(JourneyChange { id: id.into(), title, state: JState::Gone, edited: vec![], added: vec![], removed: vec![], now_changes: vec![], no_longer_changes: vec![] }),
            (Some(&x), Some(&y)) => {
                let (sa, sb) = (steps(a, x, &moved), steps(b, y, &BTreeMap::new()));
                let edited: Vec<String> = sb.iter().filter(|(k, v)| sa.get(*k).map(|w| w.1 != v.1).unwrap_or(false)).map(|(_, v)| step_label(names, b, v.0)).collect();
                let added: Vec<String> = sb.iter().filter(|(k, _)| !sa.contains_key(*k)).map(|(_, v)| step_label(names, b, v.0)).collect();
                let removed: Vec<String> = sa.iter().filter(|(k, _)| !sb.contains_key(*k)).map(|(_, v)| step_label(names, a, v.0)).collect();
                let (wa, wb) = (writes(a, x), writes(b, y));
                let now_changes: Vec<String> = wb.difference(&wa).map(|k| key_label(k)).collect();
                let no_longer_changes: Vec<String> = wa.difference(&wb).map(|k| key_label(k)).collect();
                if edited.is_empty() && added.is_empty() && removed.is_empty() && now_changes.is_empty() && no_longer_changes.is_empty() {
                    rep.untouched += 1;
                } else {
                    rep.journeys.push(JourneyChange { id: id.into(), title, state: JState::Changed, edited, added, removed, now_changes, no_longer_changes });
                }
            }
            (None, None) => {}
        }
    }

    let moved_to: BTreeMap<&String, &String> = moved.iter().map(|(k, v)| (v, k)).collect();
    for (sid, (r, h, sh)) in &lb {
        let state = match la.get(sid) {
            Some((_, h0, s0)) if s0 != sh => "edited".to_string(),
            Some((_, h0, _)) if h0 != h => "reformatted only".to_string(),
            Some(_) => continue,
            None => match moved_to.get(sid) {
                Some(from) => {
                    let (_, _, s0) = &la[*from];
                    let place = from.split('#').next().unwrap_or("");
                    let renamed = name(from) != name(sid);
                    let what = if renamed { format!("renamed from {}", name(from)) } else { format!("moved from {place}") };
                    if s0 == sh { what } else { format!("{what} and edited") }
                }
                None => "new".to_string(),
            },
        };
        rep.functions.push(FnChange { sid: sid.clone(), label: step_label(names, b, *r), state, journeys: journeys_using(b, names, *r) });
    }
    for (sid, (r, ..)) in &la {
        if !lb.contains_key(sid) && !moved.contains_key(sid) {
            rep.functions.push(FnChange { sid: sid.clone(), label: step_label(names, a, *r), state: "gone".into(), journeys: journeys_using(a, names, *r) });
        }
    }

    let support = |l: &Lift| -> BTreeMap<String, String> {
        l.repo.files.iter().enumerate().map(|(fi, f)| (f.path.clone(), hash_of(&l.repo, (fi, SUPPORT)))).collect()
    };
    let (pa, pb) = (support(a), support(b));
    for (p, h) in &pb {
        if pa.get(p).map(|x| x != h).unwrap_or(false) {
            rep.setup_changed.push(p.clone());
        }
    }
    for name in a.repo.packages.keys().chain(b.repo.packages.keys()).collect::<BTreeSet<_>>() {
        let (from, to) = (a.repo.packages.get(name), b.repo.packages.get(name));
        if from != to {
            let journeys = b.flows.iter().filter(|f| f.outside.contains(name)).map(|f| journey_title(names, &f.id)).collect();
            rep.packages.push(PackageChange { name: name.clone(), from: from.cloned(), to: to.cloned(), journeys });
        }
    }
    // The ones journeys use first.
    rep.packages.sort_by_key(|p| (p.journeys.is_empty(), p.name.clone()));
    let ua: BTreeSet<String> = a.out.stats.unplaced.iter().map(|u| format!("{}  {}", u.0, u.2)).collect();
    rep.new_unplaced = b.out.stats.unplaced.iter().map(|u| format!("{}  {}", u.0, u.2)).filter(|k| !ua.contains(k)).collect();
    rep
}

fn list(v: &[String], max: usize) -> String {
    if v.len() <= max {
        v.join(", ")
    } else {
        format!("{} and {} more", v[..max].join(", "), v.len() - max)
    }
}

/// The report as plain sentences, for the terminal and for commit messages.
pub fn render(r: &Report) -> String {
    let mut o = String::new();
    if r.is_empty() {
        return "Nothing changed.\n".into();
    }
    let changed: Vec<&JourneyChange> = r.journeys.iter().filter(|j| j.state == JState::Changed).collect();
    if !changed.is_empty() {
        o += &format!("{} {} differently:\n", changed.len(), if changed.len() == 1 { "journey works" } else { "journeys work" });
        for j in &changed {
            o += &format!("  {}\n", j.title);
            if !j.edited.is_empty() {
                o += &format!("      changed: {}\n", list(&j.edited, 4));
            }
            if !j.added.is_empty() {
                o += &format!("      now also: {}\n", list(&j.added, 4));
            }
            if !j.removed.is_empty() {
                o += &format!("      no longer: {}\n", list(&j.removed, 4));
            }
            if !j.now_changes.is_empty() {
                o += &format!("      now changes {}\n", list(&j.now_changes, 4));
            }
            if !j.no_longer_changes.is_empty() {
                o += &format!("      no longer changes {}\n", list(&j.no_longer_changes, 4));
            }
        }
    }
    for (state, word) in [(JState::New, "New journey"), (JState::Gone, "Journey removed")] {
        for j in r.journeys.iter().filter(|j| j.state == state) {
            o += &format!("{word}: {}\n", j.title);
        }
    }
    if r.untouched > 0 {
        o += &format!("{} other {} untouched.\n", r.untouched, if r.untouched == 1 { "journey is" } else { "journeys are" });
    }
    if !r.functions.is_empty() {
        o += "\nWhat changed in the code:\n";
        for f in &r.functions {
            let used = if f.journeys.is_empty() { "no journey runs it".to_string() } else { format!("in {}", list(&f.journeys, 3)) };
            o += &format!("  {} ({})  {}\n      {}\n", f.label, f.state, f.sid.split('#').next().unwrap_or(""), used);
        }
    }
    if !r.setup_changed.is_empty() {
        o += &format!("\nImports or setup changed in: {}\n", list(&r.setup_changed, 5));
    }
    if !r.new_unplaced.is_empty() {
        o += "\nNew code that belongs to no journey:\n";
        for u in &r.new_unplaced {
            o += &format!("  {u}\n");
        }
    }
    if !r.packages.is_empty() {
        o += &format!("\n{} changed version:\n", if r.packages.len() == 1 { "1 package".to_string() } else { format!("{} packages", r.packages.len()) });
        for p in r.packages.iter().take(15) {
            let v = match (&p.from, &p.to) {
                (Some(a), Some(b)) => format!("{a} -> {b}"),
                (None, Some(b)) => format!("added at {b}"),
                (Some(a), None) => format!("removed (was {a})"),
                _ => String::new(),
            };
            let used = if p.journeys.is_empty() { String::new() } else { format!("\n      used by {}", list(&p.journeys, 3)) };
            o += &format!("  {} {v}{used}\n", p.name);
        }
        if r.packages.len() > 15 {
            o += &format!("  and {} more\n", r.packages.len() - 15);
        }
    }
    if !r.other_files.is_empty() {
        o += &format!("\nOther files changed (not read as journeys): {}\n", list(&r.other_files, 8));
    }
    o
}

/// One line for the log: which journeys moved.
pub fn one_line(r: &Report) -> String {
    let names: Vec<String> = r
        .journeys
        .iter()
        .map(|j| match j.state {
            JState::New => format!("new: {}", j.title),
            JState::Gone => format!("removed: {}", j.title),
            JState::Changed => j.title.clone(),
        })
        .collect();
    if !names.is_empty() {
        return list(&names, 3);
    }
    if !r.functions.is_empty() {
        return format!("code no journey runs: {}", list(&r.functions.iter().map(|f| f.label.clone()).collect::<Vec<_>>(), 3));
    }
    if !r.setup_changed.is_empty() || !r.other_files.is_empty() {
        return "no journey changed".into();
    }
    "nothing changed".into()
}
