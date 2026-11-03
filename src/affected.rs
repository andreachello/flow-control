//! Which journeys a change touched: lift two snapshots and compare them flow by flow.
//!
//! This is the base for everything that decides what can merge: two changes whose affected flows
//! (and the state those flows change) do not overlap cannot change each other's journeys, so they
//! can merge in parallel, and each one only needs the scenarios of its own flows.

use crate::output::hash_of;
use crate::parse::Owner;
use crate::Lift;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

struct Snap {
    /// (slice id, hash) in call order.
    steps: Vec<(String, String)>,
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
}

fn snapshot(l: &Lift) -> BTreeMap<String, Snap> {
    l.flows
        .iter()
        .map(|f| {
            let mut s = Snap { steps: vec![], reads: BTreeSet::new(), writes: BTreeSet::new() };
            for st in &f.steps {
                s.steps.push((l.repo.sid(st.at), hash_of(&l.repo, st.at)));
                if let Some(r) = l.graph.reads.get(&st.at) {
                    s.reads.extend(r.iter().cloned());
                }
                if let Some(w) = l.graph.writes.get(&st.at) {
                    s.writes.extend(w.iter().cloned());
                }
            }
            (f.id.clone(), s)
        })
        .collect()
}

/// Every slice in the repo and its hash.
fn all_slices(l: &Lift) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (fi, f) in l.repo.files.iter().enumerate() {
        for si in 0..f.slices.len() {
            m.insert(l.repo.sid((fi, si)), hash_of(&l.repo, (fi, si)));
        }
    }
    m
}

/// The code in each file that runs when it loads and belongs to no slice.
fn unplaced(l: &Lift) -> BTreeMap<String, String> {
    l.repo
        .files
        .iter()
        .filter_map(|f| {
            // The owned lines in full: `Unplaced::text` is shortened for display.
            let text: Vec<&str> = f.lines.iter().zip(&f.owner).filter(|(_, o)| matches!(o, Owner::Unplaced(_))).map(|(l, _)| l.as_str()).collect();
            (!text.is_empty()).then(|| (f.path.clone(), text.join("\n")))
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Serialize)]
pub struct FlowChange {
    pub id: String,
    pub how: How,
    /// Steps in both snapshots whose code changed.
    pub edited: Vec<String>,
    /// Steps the flow now runs that it did not run before.
    pub entered: Vec<String>,
    /// Steps the flow no longer runs.
    pub left: Vec<String>,
    /// Same steps, different order.
    pub reordered: bool,
    /// State the flow reads or changes, from either snapshot: what a collision check compares.
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub flows: Vec<FlowChange>,
    /// Flows present in both snapshots that did not change.
    pub unchanged: usize,
    /// Slices that changed, appeared or disappeared but that no flow runs in either snapshot.
    pub loose: Vec<String>,
    /// Files whose load-time code (no owner) changed. Its effect cannot be pinned to a flow.
    pub unplaced: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.flows.is_empty() && self.loose.is_empty() && self.unplaced.is_empty()
    }

    /// True when a reviewer or merge queue cannot rely on the flow list alone.
    pub fn uncertain(&self) -> bool {
        !self.unplaced.is_empty()
    }
}

pub fn affected(base: &Lift, head: &Lift) -> Report {
    let (a, b) = (snapshot(base), snapshot(head));
    let mut flows = vec![];
    let mut unchanged = 0;
    let ids: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for id in ids {
        let (fa, fb) = (a.get(id), b.get(id));
        let reads = fa.iter().chain(fb.iter()).flat_map(|s| s.reads.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
        let writes = fa.iter().chain(fb.iter()).flat_map(|s| s.writes.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
        let sids = |s: &Snap| s.steps.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        let change = match (fa, fb) {
            (None, Some(s)) => FlowChange { id: id.clone(), how: How::Added, edited: vec![], entered: sids(s), left: vec![], reordered: false, reads, writes },
            (Some(s), None) => FlowChange { id: id.clone(), how: How::Removed, edited: vec![], entered: vec![], left: sids(s), reordered: false, reads, writes },
            (Some(x), Some(y)) => {
                let hx: BTreeMap<&String, &String> = x.steps.iter().map(|(k, h)| (k, h)).collect();
                let hy: BTreeMap<&String, &String> = y.steps.iter().map(|(k, h)| (k, h)).collect();
                let edited: Vec<String> = y.steps.iter().filter(|(k, h)| hx.get(k).is_some_and(|o| *o != h)).map(|(k, _)| k.clone()).collect();
                let entered: Vec<String> = y.steps.iter().filter(|(k, _)| !hx.contains_key(k)).map(|(k, _)| k.clone()).collect();
                let left: Vec<String> = x.steps.iter().filter(|(k, _)| !hy.contains_key(k)).map(|(k, _)| k.clone()).collect();
                let common = |s: &Snap, other: &BTreeMap<&String, &String>| s.steps.iter().filter(|(k, _)| other.contains_key(k)).map(|(k, _)| k.clone()).collect::<Vec<_>>();
                let reordered = common(x, &hy) != common(y, &hx);
                if edited.is_empty() && entered.is_empty() && left.is_empty() && !reordered {
                    unchanged += 1;
                    continue;
                }
                FlowChange { id: id.clone(), how: How::Changed, edited, entered, left, reordered, reads, writes }
            }
            (None, None) => unreachable!(),
        };
        flows.push(change);
    }

    let in_flows: BTreeSet<&String> = a.values().chain(b.values()).flat_map(|s| s.steps.iter().map(|(k, _)| k)).collect();
    let (sa, sb) = (all_slices(base), all_slices(head));
    let loose = sa
        .keys()
        .chain(sb.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        // Imports and file headers belong to no function; their effect shows up in the flows that resolve through them.
        .filter(|k| !in_flows.contains(k) && !k.ends_with("#(support)") && sa.get(*k) != sb.get(*k))
        .cloned()
        .collect();

    let (ua, ub) = (unplaced(base), unplaced(head));
    let unplaced = ua.keys().chain(ub.keys()).collect::<BTreeSet<_>>().into_iter().filter(|k| ua.get(*k) != ub.get(*k)).cloned().collect();

    Report { flows, unchanged, loose, unplaced }
}
