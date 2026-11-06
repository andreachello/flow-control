//! What changed between two snapshots, said in journeys and functions. No lines, no diffs.

use serde::{Deserialize, Serialize};
use crate::names::Names;
use crate::output::hash_of;
use crate::parse::SUPPORT;
use crate::util::{fnv64 as fnv, human};
use crate::Lift;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn journey_title(names: &Names, id: &str) -> String {
    names.journeys.get(id).map(|n| n.title.clone()).unwrap_or_else(|| id.to_string())
}

pub fn step_label(names: &Names, l: &Lift, r: (usize, usize)) -> String {
    names.steps.get(&l.repo.sid(r)).map(|n| n.name.clone()).unwrap_or_else(|| human(&l.repo.slice(r).name))
}

fn key_label(k: &str) -> String {
    human(k.rsplit('#').next().unwrap_or(k).trim_start_matches("S.")).to_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JState {
    New,
    Gone,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyChange {
    pub id: String,
    /// The permanent id, the same before and after a rename.
    pub uid: String,
    /// The name it had before this change, when it was renamed.
    pub was: Option<String>,
    pub title: String,
    pub state: JState,
    pub edited: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub now_changes: Vec<String>,
    pub no_longer_changes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnChange {
    pub sid: String,
    pub label: String,
    pub state: String,
    pub journeys: Vec<String>,
    /// What is different, read from the code.
    pub facts: Vec<String>,
    /// Where it lived before, when it moved or was renamed.
    pub before: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub journeys: Vec<JourneyChange>,
    pub untouched: usize,
    pub functions: Vec<FnChange>,
    pub setup_changed: Vec<String>,
    pub new_unplaced: Vec<String>,
    pub other_files: Vec<String>,
    pub packages: Vec<PackageChange>,
    /// (file, what changed in it) for data files with readable keys.
    pub data_facts: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        // Docs, data and styles are reported as whole files, not block by block.
        if crate::universal::is_plain(&f.path) {
            continue;
        }
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
        // A command's first step is the dispatcher all commands share: not a change to this one.
        .filter(|s| !(l.flows[fi].kind == "command" && s.depth == 0))
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
    l.flows.iter().filter(|f| f.steps.iter().any(|s| s.at == r && !(f.kind == "command" && s.depth == 0))).map(|f| journey_title(names, &f.id)).collect()
}

/// The constants a journey's steps read. A constant is not a step, since nothing calls it, but
/// changing one changes what every journey reading it does. Found by name through the same
/// imports calls resolve through, and followed through constants that read other constants.
pub fn constants(l: &Lift, fi: usize) -> Vec<(usize, usize)> {
    constants_shared(l, fi, &mut HashMap::new())
}

/// The same, with what each function reads remembered in `read`. Most journeys share most of
/// their functions, so asking about every journey would otherwise rescan the same code thousands
/// of times.
fn constants_shared(l: &Lift, fi: usize, read: &mut HashMap<(usize, usize), Vec<(usize, usize)>>) -> Vec<(usize, usize)> {
    use crate::repo::Res;
    static WORD: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let word = WORD.get_or_init(|| regex::Regex::new(r"[A-Za-z_$][\w$]*").unwrap());
    let steps: BTreeSet<(usize, usize)> = l.flows[fi].steps.iter().map(|s| s.at).collect();
    let mut todo: Vec<(usize, usize)> = steps.iter().copied().collect();
    let (mut seen, mut out) = (steps.clone(), BTreeSet::new());
    while let Some(r) = todo.pop() {
        let data = read.entry(r).or_insert_with(|| {
            let (f, sl) = (&l.repo.files[r.0], l.repo.slice(r));
            let text = sl.text(f);
            let names: BTreeSet<&str> = word.find_iter(&text).map(|m| m.as_str()).collect();
            let hits = names.into_iter().filter_map(|n| match l.repo.resolve_ident(r.0, n, &sl.locals) {
                Res::Slice(hit) if l.repo.slice(hit).role == crate::parse::Role::Data => Some(hit),
                _ => None,
            });
            hits.collect()
        });
        for &hit in data.iter() {
            if seen.insert(hit) {
                out.insert(hit);
                todo.push(hit);
            }
        }
    }
    out.into_iter().collect()
}

/// Compare snapshot `a` (before) with `b` (after).
pub fn compare(a: &Lift, b: &Lift, names: &Names, other_files: Vec<String>) -> Report {
    let mut rep = Report { other_files, ..Default::default() };
    // Paired by permanent id, so a renamed journey is the same journey. Listed by today's name.
    let ai: BTreeMap<&str, usize> = a.flows.iter().enumerate().map(|(i, f)| (f.uid.as_str(), i)).collect();
    let bi: BTreeMap<&str, usize> = b.flows.iter().enumerate().map(|(i, f)| (f.uid.as_str(), i)).collect();
    let mut ids: Vec<(&str, &str)> = bi.iter().map(|(u, &i)| (b.flows[i].id.as_str(), *u)).chain(ai.iter().filter(|(u, _)| !bi.contains_key(*u)).map(|(u, &i)| (a.flows[i].id.as_str(), *u))).collect();
    ids.sort();

    // Functions that moved or were renamed: gone in one place, new in another, same name in another
    // file or the same behaviour under another name.
    let (la, lb) = (slices(a), slices(b));
    let gone: Vec<&String> = la.keys().filter(|k| !lb.contains_key(*k)).collect();
    let new: Vec<&String> = lb.keys().filter(|k| !la.contains_key(*k)).collect();
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let name = |sid: &str| sid.rsplit('#').next().unwrap_or("").to_string();
    // What the permanent ids already tie together needs no looking for.
    let by_uid: BTreeMap<&str, &String> = new.iter().map(|n| (b.uids.step(n), *n)).collect();
    for g in &gone {
        if let Some(n) = by_uid.get(a.uids.step(g)) {
            moved.insert((*g).clone(), (*n).clone());
        }
    }
    let unlinked: Vec<&&String> = gone.iter().filter(|g| !moved.contains_key(**g)).collect();
    for g in unlinked {
        let (_, _, gs) = &la[*g];
        let hit = new
            .iter()
            .find(|n| !moved.values().any(|v| v == **n) && name(n) == name(g) && new.iter().filter(|m| name(m) == name(g)).count() == 1)
            .or_else(|| new.iter().find(|n| !moved.values().any(|v| v == **n) && &lb[**n].2 == gs && !gs.is_empty()));
        if let Some(n) = hit {
            moved.insert((*g).clone(), (*n).clone());
        }
    }
    // Constants no journey runs but some read. Worth the search only when one changed.
    let in_flow: BTreeSet<(usize, usize)> = b.flows.iter().flat_map(|f| f.steps.iter().map(|s| s.at)).collect();
    let edited_constants: BTreeSet<(usize, usize)> = lb
        .iter()
        .filter(|(sid, (r, _, sh))| b.repo.slice(*r).role == crate::parse::Role::Data && !in_flow.contains(r) && la.get(*sid).is_some_and(|o| &o.2 != sh))
        .map(|(_, (r, ..))| *r)
        .collect();
    let mut read_by: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    if !edited_constants.is_empty() {
        let mut read = HashMap::new();
        for (fi, f) in b.flows.iter().enumerate() {
            let hit: Vec<(usize, usize)> = constants_shared(b, fi, &mut read).into_iter().filter(|r| edited_constants.contains(r)).collect();
            if !hit.is_empty() {
                read_by.insert(&f.id, hit);
            }
        }
    }
    for (id, uid) in ids {
        let title = journey_title(names, id);
        match (ai.get(uid), bi.get(uid)) {
            (None, Some(_)) => rep.journeys.push(JourneyChange { id: id.into(), uid: uid.into(), was: None, title, state: JState::New, edited: vec![], added: vec![], removed: vec![], now_changes: vec![], no_longer_changes: vec![] }),
            (Some(_), None) => rep.journeys.push(JourneyChange { id: id.into(), uid: uid.into(), was: None, title, state: JState::Gone, edited: vec![], added: vec![], removed: vec![], now_changes: vec![], no_longer_changes: vec![] }),
            (Some(&x), Some(&y)) => {
                let (sa, sb) = (steps(a, x, &moved), steps(b, y, &BTreeMap::new()));
                let mut edited: Vec<String> = sb.iter().filter(|(k, v)| sa.get(*k).map(|w| w.1 != v.1).unwrap_or(false)).map(|(_, v)| step_label(names, b, v.0)).collect();
                edited.extend(read_by.get(id).into_iter().flatten().map(|r| step_label(names, b, *r)));
                let added: Vec<String> = sb.iter().filter(|(k, _)| !sa.contains_key(*k)).map(|(_, v)| step_label(names, b, v.0)).collect();
                let removed: Vec<String> = sa.iter().filter(|(k, _)| !sb.contains_key(*k)).map(|(_, v)| step_label(names, a, v.0)).collect();
                let (wa, wb) = (writes(a, x), writes(b, y));
                let now_changes: Vec<String> = wb.difference(&wa).map(|k| key_label(k)).collect();
                let no_longer_changes: Vec<String> = wa.difference(&wb).map(|k| key_label(k)).collect();
                let was = Some(a.flows[x].id.clone()).filter(|w| w != id);
                if was.is_none() && edited.is_empty() && added.is_empty() && removed.is_empty() && now_changes.is_empty() && no_longer_changes.is_empty() {
                    rep.untouched += 1;
                } else {
                    rep.journeys.push(JourneyChange { id: id.into(), uid: uid.into(), was, title, state: JState::Changed, edited, added, removed, now_changes, no_longer_changes });
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
        let facts = match (state.as_str(), la.get(sid), moved_to.get(sid)) {
            ("reformatted only", ..) => vec![],
            (_, Some((ra, ..)), _) => crate::facts::edited(a, *ra, b, *r),
            (s, None, Some(from)) if s.ends_with("and edited") => crate::facts::edited(a, la[*from].0, b, *r),
            (_, None, Some(_)) => vec![],
            _ => crate::facts::added(b, *r),
        };
        let mut journeys = journeys_using(b, names, *r);
        journeys.extend(read_by.iter().filter(|(_, v)| v.contains(r)).map(|(id, _)| journey_title(names, id)));
        rep.functions.push(FnChange { sid: sid.clone(), label: step_label(names, b, *r), state, journeys, facts, before: moved_to.get(sid).map(|s| s.to_string()) });
    }
    for (sid, (r, ..)) in &la {
        if !lb.contains_key(sid) && !moved.contains_key(sid) {
            rep.functions.push(FnChange { sid: sid.clone(), label: step_label(names, a, *r), state: "gone".into(), journeys: journeys_using(a, names, *r), facts: vec![], before: None });
        }
    }

    let support = |l: &Lift| -> BTreeMap<String, String> {
        l.repo.files.iter().enumerate().map(|(fi, f)| (f.path.clone(), hash_of(&l.repo, (fi, SUPPORT)))).collect()
    };
    let (pa, pb) = (support(a), support(b));
    for (p, h) in &pb {
        if pa.get(p).map(|x| x != h).unwrap_or(false) && !crate::universal::is_plain(p) {
            rep.setup_changed.push(p.clone());
        }
    }
    let plain = |l: &Lift| -> BTreeMap<String, String> { l.repo.files.iter().filter(|f| crate::universal::is_plain(&f.path)).map(|f| (f.path.clone(), fnv(&f.src))).collect() };
    let (fa, fb) = (plain(a), plain(b));
    for p in fa.keys().chain(fb.keys()) {
        if fa.get(p) != fb.get(p) {
            rep.other_files.push(p.clone());
        }
    }
    rep.other_files.sort();
    rep.other_files.dedup();
    let text = |l: &Lift, p: &str| l.repo.file_idx(p).map(|i| l.repo.files[i].src.clone()).unwrap_or_default();
    for p in &rep.other_files {
        let f = crate::facts::data_file(p, &text(a, p), &text(b, p));
        if !f.is_empty() {
            rep.data_facts.push((p.clone(), f));
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
            if let Some(w) = &j.was {
                o += &format!("      renamed from: {w}\n");
            }
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
            for x in &f.facts {
                o += &format!("      {x}\n");
            }
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

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

const CHANGES: usize = 8;

fn reaches(j: &[String]) -> String {
    match j.len() {
        0 => "reaches no journey".to_string(),
        1 | 2 => cut(&format!("reaches {}", j.join(", ")), 70),
        n => cut(&format!("reaches {n} journeys: {}, {}", j[0], j[1]), 70),
    }
}

const FACTS: usize = 3;

fn facts_block(o: &mut String, facts: &[String], indent: &str) {
    for f in facts.iter().take(FACTS) {
        *o += &format!("{indent}{f}\n");
    }
    if facts.len() > FACTS {
        *o += &format!("{indent}and {} more\n", facts.len() - FACTS);
    }
}

/// The short version: each change, how far it reaches, and what is actually different.
pub fn compact(r: &Report) -> String {
    if r.is_empty() {
        return "Nothing changed.\n".into();
    }
    let n = r.journeys.len();
    let mut o = format!("{} changed, {} untouched\n", if n == 1 { "1 journey".to_string() } else { format!("{n} journeys") }, r.untouched);
    for j in r.journeys.iter().filter(|j| j.was.is_some()).take(5) {
        o += &format!("Renamed: {} is now {}\n", j.was.as_deref().unwrap_or(""), j.id);
    }
    for (state, word) in [(JState::New, "New journey"), (JState::Gone, "Journey removed")] {
        for j in r.journeys.iter().filter(|j| j.state == state).take(5) {
            o += &format!("{word}: {}\n", j.title);
        }
    }
    let mut list: Vec<&FnChange> = r.functions.iter().filter(|f| f.state != "reformatted only").collect();
    let reformatted = r.functions.len() - list.len();
    let thin = |f: &&FnChange| f.facts.is_empty() || f.facts[0] == "the shape of this data changed";
    list.sort_by_key(|f| (thin(f), std::cmp::Reverse(f.journeys.len()), f.label.clone()));
    for f in list.iter().take(CHANGES) {
        let how = if f.state == "edited" { String::new() } else { format!(" ({})", f.state) };
        o += &format!("\n{}{how}   {}\n", cut(&f.label, 44), reaches(&f.journeys));
        facts_block(&mut o, &f.facts, "    ");
    }
    if list.len() > CHANGES {
        o += &format!("\nand {} smaller changes\n", list.len() - CHANGES);
    }
    let shown: BTreeSet<&String> = r.data_facts.iter().map(|d| &d.0).collect();
    for (p, facts) in r.data_facts.iter().take(4) {
        o += &format!("\n{p}\n");
        facts_block(&mut o, facts, "    ");
    }
    let mut notes = vec![];
    if reformatted > 0 {
        notes.push(format!("Reformatted only: {reformatted}"));
    }
    if !r.new_unplaced.is_empty() {
        notes.push(format!("New code no journey owns: {}", r.new_unplaced.len()));
    }
    if !r.packages.is_empty() {
        let names: Vec<String> = r.packages.iter().map(|p| p.name.clone()).collect();
        notes.push(format!("Packages changed: {}", list_of(&names)));
    }
    let others: Vec<String> = r.other_files.iter().filter(|f| !shown.contains(f)).cloned().collect();
    if !others.is_empty() {
        notes.push(format!("Other files: {}", list_of(&others)));
    }
    if !notes.is_empty() {
        o += "\n";
        for n in notes {
            o += &format!("{n}\n");
        }
    }
    o
}

fn list_of(v: &[String]) -> String {
    list(v, 4)
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
