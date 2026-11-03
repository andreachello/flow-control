//! Flow control. Version control tracks files; flow control tracks what your product does.
//!
//! The lifter reads a repo deterministically into a flow document: every line has exactly one
//! owner (or is listed as unplaced), entry points come from conventions, and each flow is the
//! ordered set of slices reachable from its entry. Anything the lifter cannot prove is reported,
//! never guessed.

pub mod affected;
pub mod config;
pub mod flows;
pub mod output;
pub mod parse;
pub mod repo;
pub mod util;
pub mod view;
pub mod names;
pub mod source;
pub mod change;
pub mod vcs;
pub mod merge;
pub mod universal;
pub mod accuracy;
pub mod packages;
pub mod facts;
pub mod scenario;

use anyhow::Result;
use std::path::Path;

pub struct Lift {
    pub repo: repo::Repo,
    pub graph: repo::Graph,
    pub flows: Vec<flows::Flow>,
    pub out: output::Lifted,
}

pub fn lift(root: &Path, out_dir: &str) -> Result<Lift> {
    lift_source(&source::Source::Dir(root.to_path_buf()), out_dir)
}

/// Lift any snapshot: the working folder, a git commit, or files in memory.
pub fn lift_source(src: &source::Source, out_dir: &str) -> Result<Lift> {
    lift_opts(src, out_dir, false)
}

/// `no_grammars` reads everything with the universal reader, to measure it against a grammar.
pub fn lift_opts(src: &source::Source, out_dir: &str, no_grammars: bool) -> Result<Lift> {
    let repo = repo::Repo::from_source_opts(src, out_dir, no_grammars)?;
    let graph = repo::Graph::build(&repo);
    let (mut entries, problems) = flows::entries(&repo);
    flows::commands(&repo, &graph, &mut entries);
    if entries.is_empty() {
        entries = flows::roots(&repo, &graph);
    }
    let flows = flows::build(&repo, &graph, &entries);
    let out = output::render(&repo, &graph, &flows, &problems);
    Ok(Lift { repo, graph, flows, out })
}

/// Generated files currently on disk in the output folder.
pub fn read_existing(dir: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let mut m = std::collections::BTreeMap::new();
    if !dir.exists() {
        return Ok(m);
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".flow.yaml") || (name.starts_with('_') && name.ends_with(".yaml")) {
            m.insert(name, std::fs::read_to_string(e.path())?);
        }
    }
    Ok(m)
}

pub struct Drift {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl Drift {
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

pub fn drift(fresh: &std::collections::BTreeMap<String, String>, disk: &std::collections::BTreeMap<String, String>) -> Drift {
    Drift {
        added: fresh.keys().filter(|k| !disk.contains_key(*k)).cloned().collect(),
        changed: fresh.iter().filter(|(k, v)| disk.get(*k).map(|d| d != *v).unwrap_or(false)).map(|(k, _)| k.clone()).collect(),
        removed: disk.keys().filter(|k| !fresh.contains_key(*k)).cloned().collect(),
    }
}

pub fn write(dir: &Path, fresh: &std::collections::BTreeMap<String, String>) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let disk = read_existing(dir)?;
    for k in disk.keys().filter(|k| !fresh.contains_key(*k)) {
        std::fs::remove_file(dir.join(k))?;
    }
    let mut n = 0;
    for (k, v) in fresh {
        if disk.get(k) != Some(v) {
            std::fs::write(dir.join(k), v)?;
            n += 1;
        }
    }
    Ok(n)
}
