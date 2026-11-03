//! Scenarios: running journeys for real, in any language.
//!
//! A scenario is a command in flow.toml that exercises a journey and prints what happened, as JSON
//! or plain text. When two branches meet in a journey, flow runs it on four real checkouts: where
//! they split, each side alone, and the merge. Each value is then compared, so "these two changes
//! meet" becomes "both decide what is charged: 240 alone, 19.2 alone, 192 together".

use crate::config::ScenarioCfg;
use crate::merge::Plan;
use crate::source::git;
use crate::vcs::Vcs;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub type Outcome = BTreeMap<String, String>;

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut Outcome) {
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

/// What a command printed, as named values.
pub fn read_outcome(stdout: &str) -> Outcome {
    let t = stdout.trim();
    let mut out = BTreeMap::new();
    // The whole output, or its last line so logs may come first.
    for candidate in [t, t.lines().last().unwrap_or("")] {
        if let Ok(v @ serde_json::Value::Object(_)) = serde_json::from_str::<serde_json::Value>(candidate) {
            flatten("", &v, &mut out);
            return out;
        }
    }
    out.insert("output".into(), t.to_string());
    out
}

/// Run one scenario in a folder. A failure is an outcome like any other.
pub fn run(dir: &Path, s: &ScenarioCfg, side: &str) -> Result<Outcome> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&s.run)
        .current_dir(dir)
        .env("FLOW_SIDE", side)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let a = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let b = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let limit = Duration::from_secs(s.timeout.unwrap_or(120));
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&a.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&b.join().unwrap_or_default()).to_string();
    let mut out = read_outcome(&stdout);
    match status {
        None => {
            out.insert("failed".into(), format!("timed out after {}s", limit.as_secs()));
        }
        Some(st) if !st.success() => {
            out.insert("failed".into(), format!("exit {}: {}", st.code().unwrap_or(-1), stderr.trim().lines().last().unwrap_or("")));
        }
        _ => {}
    }
    Ok(out)
}

/// A real checkout of one snapshot, removed when dropped. Installed dependencies are shared from
/// the repo so scenarios run without reinstalling.
pub struct Tree {
    pub path: PathBuf,
    root: PathBuf,
}

static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Tree {
    pub fn new(root: &Path, rev: &str) -> Result<Tree> {
        let path = std::env::temp_dir().join(format!("flow-tree-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        git(root, &["worktree", "add", "-q", "--detach", &path.to_string_lossy(), rev])?;
        for d in ["node_modules", ".venv", "venv", "vendor", "target", ".env", ".env.local"] {
            let (from, to) = (root.join(d), path.join(d));
            if from.exists() && !to.exists() {
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
        }
        Ok(Tree { path, root: root.to_path_buf() })
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = git(&self.root, &["worktree", "remove", "--force", &self.path.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = git(&self.root, &["worktree", "prune"]);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Both sides changed this value, to different things.
    BothDecide,
    /// The merged value is not what either side alone produces.
    OnlyTogether,
    /// The merge fails where neither side did.
    Broken,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Finding {
    pub key: String,
    pub kind: Kind,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub merged: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Check {
    pub scenario: String,
    pub journey: Option<String>,
    pub findings: Vec<Finding>,
    /// Values that changed between two identical runs, so they prove nothing.
    pub unstable: Vec<String>,
    /// What the merge produced, kept so a person can accept it.
    pub merged: Outcome,
    /// The merge matches an outcome a person recorded earlier.
    pub matches_recorded: bool,
}

impl Check {
    pub fn clean(&self) -> bool {
        self.findings.is_empty() || self.matches_recorded
    }
}

pub fn recorded_path(root: &Path, out: &str, name: &str) -> PathBuf {
    root.join(out).join("outcomes").join(format!("{name}.json"))
}

pub fn recorded(root: &Path, out: &str, name: &str) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(recorded_path(root, out, name)).ok()?).ok()
}

pub fn record(root: &Path, out: &str, name: &str, o: &Outcome) -> Result<()> {
    let p = recorded_path(root, out, name);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, serde_json::to_string_pretty(o)? + "\n")?;
    Ok(())
}

/// Compare one value across the four runs.
pub fn judge(key: &str, b: Option<&String>, o: Option<&String>, t: Option<&String>, m: Option<&String>) -> Option<Finding> {
    let mk = |kind| Some(Finding { key: key.to_string(), kind, base: b.cloned(), ours: o.cloned(), theirs: t.cloned(), merged: m.cloned() });
    if key == "failed" {
        return if m.is_some() && o.is_none() && t.is_none() { mk(Kind::Broken) } else { None };
    }
    let (co, ct) = (o != b, t != b);
    if co && ct && o != t {
        return mk(Kind::BothDecide);
    }
    let predicted = if co { o } else if ct { t } else { b };
    if m != predicted {
        return mk(Kind::OnlyTogether);
    }
    None
}

/// Run the scenarios for the journeys two sides meet in, on real checkouts of all four states.
pub fn check_merge(v: &Vcs, p: &Plan, base: &str) -> Result<Vec<Check>> {
    let cfg = crate::config::load(&v.root)?;
    let wanted: Vec<&ScenarioCfg> = cfg
        .scenario
        .iter()
        .filter(|s| match &s.journey {
            None => true,
            Some(j) => p.meet_ids.iter().any(|id| id == j) || p.meets.iter().any(|m| &m.0 == j),
        })
        .collect();
    if wanted.is_empty() || base.is_empty() {
        return Ok(vec![]);
    }
    // The merge is already a git tree, so the fourth checkout is exactly what would be saved.
    let merged = crate::merge::commit_tree(v, &p.tree, &[&p.ours, &p.theirs])?;
    let (tb, to, tt, tm) = (Tree::new(&v.root, base)?, Tree::new(&v.root, &p.ours)?, Tree::new(&v.root, &p.theirs)?, Tree::new(&v.root, &merged)?);
    let mut out = vec![];
    for s in wanted {
        let (b, b2) = (run(&tb.path, s, "base")?, run(&tb.path, s, "base")?);
        let (o, t, m) = (run(&to.path, s, "ours")?, run(&tt.path, s, "theirs")?, run(&tm.path, s, "merged")?);
        let keys: BTreeSet<&String> = b.keys().chain(o.keys()).chain(t.keys()).chain(m.keys()).chain(b2.keys()).collect();
        let unstable: Vec<String> = keys.iter().filter(|k| b.get(**k) != b2.get(**k)).map(|k| k.to_string()).collect();
        let findings: Vec<Finding> = keys.iter().filter(|k| !unstable.contains(k)).filter_map(|k| judge(k, b.get(*k), o.get(*k), t.get(*k), m.get(*k))).collect();
        let matches_recorded = recorded(&v.root, &v.out, &s.name).map(|r| r.iter().filter(|(k, _)| !unstable.contains(k)).all(|(k, val)| m.get(k) == Some(val))).unwrap_or(false);
        out.push(Check { scenario: s.name.clone(), journey: s.journey.clone(), findings, unstable, merged: m, matches_recorded });
    }
    Ok(out)
}

fn show(v: &Option<String>) -> String {
    v.clone().map(|s| s.trim_matches('"').to_string()).unwrap_or_else(|| "nothing".into())
}

pub fn render(c: &Check, theirs: &str) -> String {
    let mut o = format!("  {}{}\n", c.scenario, c.journey.as_ref().map(|j| format!("  ({j})")).unwrap_or_default());
    if c.findings.is_empty() {
        o += "      ran on all four versions: everything is what the two changes predict\n";
    }
    for f in &c.findings {
        let word = match f.kind {
            Kind::BothDecide => "both sides decide this",
            Kind::OnlyTogether => "appears only when merged",
            Kind::Broken => "the merge fails, neither side did",
        };
        o += &format!("      {}: {word}\n", f.key);
        o += &format!("        where they split  {}\n        here              {}\n        {:<17} {}\n        merged            {}\n", show(&f.base), show(&f.ours), theirs, show(&f.theirs), show(&f.merged));
    }
    if c.matches_recorded && !c.findings.is_empty() {
        o += "      the merged result matches the outcome a person recorded, so this is accepted\n";
    }
    if !c.unstable.is_empty() {
        o += &format!("      ignored, different on every run: {}\n", c.unstable.join(", "));
    }
    o
}
