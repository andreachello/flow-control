//! Permanent ids for journeys and steps, so a rename continues a history instead of ending one.
//!
//! A journey's id is the hash of the name it was born with, so every clone and every branch gets
//! the same id with nothing stored. `flow.ids.jsonl` holds only the exceptions: one line for each
//! name a thing has now that differs from its birth name, and one for each name it gave up. A repo
//! that never renamed anything has no such file. An id is still a pure function of the commit, so
//! lifts stay byte for byte repeatable and two branches adding the same journey agree on its id.
//!
//! A rename is recorded when a lift compares the working folder with the last saved snapshot. It
//! is recorded only when the code proves it. Anything else stays one journey gone and one new, is
//! reported, and can be settled with `flow rename`.

use crate::repo::SRef;
use crate::Lift;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const FILE: &str = "flow.ids.jsonl";

const JOURNEY: &str = "j_";
const STEP: &str = "s_";

/// One fact: `key` is what this id is called now, `was` a name it gave up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Line {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    pub lines: BTreeSet<Line>,
}

/// Which id each live journey and step has in one snapshot.
#[derive(Clone, Debug, Default)]
pub struct Uids {
    pub journeys: BTreeMap<String, String>,
    pub steps: BTreeMap<String, String>,
}

impl Uids {
    pub fn step(&self, sid: &str) -> &str {
        self.steps.get(sid).map(String::as_str).unwrap_or("")
    }
}

fn born(tag: &str, key: &str, n: usize) -> String {
    let seed = if n == 0 { key.to_string() } else { format!("{key}\0{n}") };
    format!("{tag}{}", crate::util::fnv64(&seed))
}

impl Ids {
    /// A line that cannot be read is reported and ignored, so a bad edit never stops a lift.
    pub fn parse(text: &str) -> (Ids, Vec<String>) {
        let (mut lines, mut bad) = (BTreeSet::new(), vec![]);
        for (i, l) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            match serde_json::from_str::<Line>(l) {
                Ok(x) if x.key.is_some() != x.was.is_some() && (x.id.starts_with(JOURNEY) || x.id.starts_with(STEP)) => {
                    lines.insert(x);
                }
                _ => bad.push(format!("{FILE}:{} could not be read, so it was ignored", i + 1)),
            }
        }
        (Ids { lines }, bad)
    }

    /// One line per fact in a fixed order, so the same facts are always the same bytes.
    pub fn render(&self) -> String {
        self.lines.iter().map(|l| serde_json::to_string(l).unwrap() + "\n").collect()
    }

    fn recorded(&self, tag: &str, key: &str) -> Vec<&str> {
        self.lines.iter().filter(|l| l.id.starts_with(tag) && l.key.as_deref() == Some(key)).map(|l| l.id.as_str()).collect()
    }

    fn assign(&self, tag: &str, live: BTreeSet<&str>, what: &str, problems: &mut Vec<String>) -> BTreeMap<String, String> {
        let claimed: HashSet<&str> = self.lines.iter().map(|l| l.id.as_str()).collect();
        let (mut out, mut held) = (BTreeMap::new(), HashSet::new());
        // Recorded names first: a birth id is only free if no name recorded here took it.
        for k in &live {
            let ids = self.recorded(tag, k);
            if ids.len() > 1 {
                problems.push(format!("{FILE}: {what} {k:?} continues {} different histories, so the first was kept", ids.len()));
            }
            let Some(id) = ids.first() else { continue };
            if held.insert(id.to_string()) {
                out.insert(k.to_string(), id.to_string());
            } else {
                problems.push(format!("{FILE}: two {what}s continue the same history, so {k:?} starts a new one. Run `flow rename` to choose"));
            }
        }
        for k in live.iter().filter(|k| !out.contains_key(**k)).copied().collect::<Vec<_>>() {
            let id = (0..).map(|n| born(tag, k, n)).find(|c| !claimed.contains(c.as_str()) && !held.contains(c)).unwrap();
            held.insert(id.clone());
            out.insert(k.to_string(), id);
        }
        out
    }

    pub fn resolve(&self, journeys: BTreeSet<&str>, steps: BTreeSet<&str>, problems: &mut Vec<String>) -> Uids {
        Uids { journeys: self.assign(JOURNEY, journeys, "journey", problems), steps: self.assign(STEP, steps, "step", problems) }
    }

    /// The id a journey's name stands for, whether it is the name in use or one it gave up.
    pub fn journey(&self, name: &str) -> String {
        let id = self.recorded(JOURNEY, name).first().map(|s| s.to_string());
        id.or_else(|| self.was(name).into_iter().find(|i| i.starts_with(JOURNEY)).map(str::to_string)).unwrap_or_else(|| born(JOURNEY, name, 0))
    }

    /// The history a name that is no longer in use now continues under.
    pub fn was(&self, tag_key: &str) -> Vec<&str> {
        self.lines.iter().filter(|l| l.was.as_deref() == Some(tag_key)).map(|l| l.id.as_str()).collect()
    }

    /// The id a name has or had, whether or not anything still uses it.
    fn of(&self, tag: &str, key: &str) -> String {
        self.recorded(tag, key).first().map(|s| s.to_string()).or_else(|| self.was(key).into_iter().find(|i| i.starts_with(tag)).map(str::to_string)).unwrap_or_else(|| born(tag, key, 0))
    }

    /// `new` now carries `id`, which `old` carried. Going back to an earlier name undoes the old fact.
    pub fn rename(&mut self, id: &str, old: &str, new: &str) {
        let tag = &id[..2];
        self.lines.retain(|l| !(l.id.starts_with(tag) && l.key.as_deref() == Some(new)) && !(l.id == id && (l.key.as_deref() == Some(old) || l.was.as_deref() == Some(new))));
        self.lines.insert(Line { id: id.to_string(), key: Some(new.to_string()), was: None });
        self.lines.insert(Line { id: id.to_string(), key: None, was: Some(old.to_string()) });
    }

    /// Say by hand that `new` continues `old`. Returns false when neither is a name in use.
    pub fn link(&mut self, l: &Lift, old: &str, new: &str) -> bool {
        let tag = if l.uids.journeys.contains_key(new) {
            JOURNEY
        } else if l.uids.steps.contains_key(new) {
            STEP
        } else {
            return false;
        };
        let id = self.of(tag, old);
        self.rename(&id, old, new);
        true
    }
}

/// Every set of facts over one base: what either side added stays, what either side removed goes.
pub fn merge(base: &str, ours: &str, theirs: &str) -> String {
    let set = |t: &str| Ids::parse(t).0.lines;
    let (b, o, t) = (set(base), set(ours), set(theirs));
    let lines = o.union(&t).filter(|l| !b.contains(*l) || (o.contains(*l) && t.contains(*l))).cloned().collect();
    Ids { lines }.render()
}

/// What a lift found by comparing the working folder with the last saved snapshot.
#[derive(Debug, Default)]
pub struct Found {
    pub renamed: Vec<(String, String)>,
    pub moved: usize,
    /// Journeys gone and journeys new with nothing in the code to tie them together.
    pub gone: Vec<String>,
    pub new: Vec<String>,
    pub changed: bool,
}

/// Code with its own name taken out and spacing evened, so a pure rename compares equal.
fn body(l: &Lift, r: SRef) -> String {
    let s = l.repo.slice(r);
    let short = s.name.rsplit('.').next().unwrap_or(&s.name);
    s.text(&l.repo.files[r.0]).replace(short, "").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lines_of(l: &Lift, r: SRef) -> BTreeSet<String> {
    l.repo.slice(r).text(&l.repo.files[r.0]).lines().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// At least half of all the lines the two share, so a shared name alone proves nothing: every
/// route handler in a repo is called POST.
fn similar(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    let both = a.intersection(b).count();
    both > 0 && both * 2 >= a.union(b).count()
}

fn only_pairs<K: Ord + Clone, V: Clone>(a: &[(K, V)], b: &[(K, V)]) -> Vec<(V, V)> {
    let group = |v: &[(K, V)]| {
        let mut m: BTreeMap<K, Vec<V>> = BTreeMap::new();
        for (k, x) in v {
            m.entry(k.clone()).or_default().push(x.clone());
        }
        m
    };
    let (ga, gb) = (group(a), group(b));
    ga.into_iter().filter_map(|(k, x)| Some((x, gb.get(&k)?))).filter(|(x, y)| x.len() == 1 && y.len() == 1).map(|(x, y)| (x[0].clone(), y[0].clone())).collect()
}

/// Steps that lost their old name and steps with a name that is new, tied together only when the
/// match is the only one possible on both sides.
fn step_links(before: &Lift, now: &Lift) -> Vec<(String, String)> {
    let code = |l: &Lift| l.repo.files.iter().enumerate().flat_map(|(fi, f)| (0..f.slices.len()).filter(move |&si| si != crate::parse::SUPPORT && !f.slices[si].lines.is_empty() && !crate::universal::is_plain(&f.path)).map(move |si| (fi, si))).collect::<Vec<SRef>>();
    let (ids_b, ids_n): (HashSet<&String>, HashSet<&String>) = (before.uids.steps.values().collect(), now.uids.steps.values().collect());
    let mut gone: Vec<(String, SRef)> = code(before).into_iter().map(|r| (before.repo.sid(r), r)).filter(|(s, _)| !ids_n.contains(&before.uids.steps[s])).collect();
    let mut new: Vec<(String, SRef)> = code(now).into_iter().map(|r| (now.repo.sid(r), r)).filter(|(s, _)| !ids_b.contains(&now.uids.steps[s])).collect();
    let mut links = vec![];
    // The same code under another name is proof by itself.
    let by_body = |v: &[(String, SRef)], l: &Lift| v.iter().map(|(s, r)| (body(l, *r), s.clone())).filter(|(b, _)| !b.is_empty()).collect::<Vec<_>>();
    links.extend(only_pairs(&by_body(&gone, before), &by_body(&new, now)));
    let done_a: HashSet<String> = links.iter().map(|(a, _)| a.clone()).collect();
    let done_b: HashSet<String> = links.iter().map(|(_, b): &(String, String)| b.clone()).collect();
    gone.retain(|(s, _)| !done_a.contains(s));
    new.retain(|(s, _)| !done_b.contains(s));
    // The same name in another file, with most of the code still there.
    let by_name = |v: &[(String, SRef)]| v.iter().map(|(s, r)| (s.rsplit('#').next().unwrap_or("").to_string(), (s.clone(), *r))).collect::<Vec<_>>();
    for ((a, ra), (b, rb)) in only_pairs(&by_name(&gone), &by_name(&new)) {
        if similar(&lines_of(before, ra), &lines_of(now, rb)) {
            links.push((a, b));
        }
    }
    links.sort();
    links
}

/// Bring `now` in line with what the code proves against the last saved snapshot.
pub fn reconcile(before: &Lift, now: &mut Lift) -> Found {
    let mut ids = now.ids.clone();
    // A rename not saved yet and since undone must not linger.
    let (lj, ls) = (now.uids.journeys.clone(), now.uids.steps.clone());
    let live = |l: &Line, k: &str| if l.id.starts_with(JOURNEY) { lj.contains_key(k) } else { ls.contains_key(k) };
    ids.lines.retain(|l| before.ids.lines.contains(l) || (l.key.as_deref().is_none_or(|k| live(l, k)) && l.was.as_deref().is_none_or(|k| !live(l, k))));
    if ids != now.ids {
        crate::stamp(now, ids.clone());
    }

    let links = step_links(before, now);
    let to: HashMap<&str, &str> = links.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    for (a, b) in &links {
        ids.rename(before.uids.step(a), a, b);
    }
    let mut f = Found { moved: links.len(), ..Default::default() };
    let (uid_b, uid_n): (HashSet<&String>, HashSet<&String>) = (before.flows.iter().map(|x| &x.uid).collect(), now.flows.iter().map(|x| &x.uid).collect());
    let gone: Vec<&crate::flows::Flow> = before.flows.iter().filter(|x| !uid_n.contains(&x.uid)).collect();
    let new: Vec<&crate::flows::Flow> = now.flows.iter().filter(|x| !uid_b.contains(&x.uid)).collect();
    let (mut left, mut matched) = (vec![], BTreeSet::new());
    for g in gone {
        let sid = before.repo.sid(g.entry);
        let want = to.get(sid.as_str()).map(|s| s.to_string()).unwrap_or(sid);
        let hits: Vec<_> = new.iter().filter(|n| now.repo.sid(n.entry) == want).collect();
        match hits.as_slice() {
            [n] => {
                ids.rename(&g.uid, &g.id, &n.id);
                matched.insert(n.id.clone());
                f.renamed.push((g.id.clone(), n.id.clone()));
            }
            _ => left.push(g.id.clone()),
        }
    }
    let unmatched: Vec<String> = new.iter().filter(|n| !matched.contains(&n.id)).map(|n| n.id.clone()).collect();
    if !left.is_empty() && !unmatched.is_empty() {
        f.gone = left;
        f.new = unmatched;
    }
    if ids != now.ids {
        crate::stamp(now, ids);
    }
    f.changed = now.ids != before.ids;
    f
}
