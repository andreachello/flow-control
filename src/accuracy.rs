//! Measure the universal reader against a grammar reader on the same code.
//!
//! TypeScript's reader follows real imports, so it can act as the answer key: lift the repo twice,
//! once normally and once pretending no grammar exists, then compare what each journey reaches.

use crate::repo::{Graph, Repo, SRef};
use crate::source::Source;
use crate::Lift;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

fn reach(repo: &Repo, g: &Graph, start: SRef) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    let mut out = BTreeSet::new();
    while let Some(r) = stack.pop() {
        if !seen.insert(r) {
            continue;
        }
        if repo.slice(r).role.runs() {
            out.insert(repo.sid(r));
        }
        if let Some(es) = g.edges.get(&r) {
            stack.extend(es.iter().map(|e| e.to));
        }
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct Score {
    pub journeys: usize,
    pub missing_entries: usize,
    pub correct: usize,
    pub extra: usize,
    pub missed: usize,
    pub exact: usize,
    pub extras: BTreeMap<String, usize>,
    pub misses: BTreeMap<String, usize>,
}

impl Score {
    pub fn precision(&self) -> f64 {
        self.correct as f64 / (self.correct + self.extra).max(1) as f64
    }
    pub fn recall(&self) -> f64 {
        self.correct as f64 / (self.correct + self.missed).max(1) as f64
    }
}

pub fn measure(root: &std::path::Path, out: &str) -> Result<Score> {
    let src = Source::Dir(root.to_path_buf());
    let truth: Lift = crate::lift_source(&src, out)?;
    let guess: Lift = crate::lift_opts(&src, out, true)?;
    let mut s = Score::default();
    for f in &truth.flows {
        let path = &truth.repo.files[f.entry.0].path;
        let name = &truth.repo.slice(f.entry).name;
        let Some(fi) = guess.repo.file_idx(path) else { continue };
        s.journeys += 1;
        let Some(&si) = guess.repo.files[fi].by_name.get(name) else {
            s.missing_entries += 1;
            continue;
        };
        let t = reach(&truth.repo, &truth.graph, f.entry);
        let g = reach(&guess.repo, &guess.graph, (fi, si));
        let (mut extra, mut missed) = (0, 0);
        for x in g.difference(&t) {
            extra += 1;
            *s.extras.entry(x.clone()).or_default() += 1;
        }
        for x in t.difference(&g) {
            missed += 1;
            *s.misses.entry(x.clone()).or_default() += 1;
        }
        s.correct += t.intersection(&g).count();
        s.extra += extra;
        s.missed += missed;
        if extra == 0 && missed == 0 {
            s.exact += 1;
        }
    }
    Ok(s)
}
