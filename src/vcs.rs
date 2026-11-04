//! Flow control on top of git's snapshots: status, save, log and journey history.
//! Git stores the files. Every question a person asks is answered in journeys.

use crate::change::{self, Report};
use crate::ids::Found;
use crate::names::Names;
use crate::universal::readable as is_source;
use crate::source::{git, Source};
use crate::{lift_source, Lift};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Vcs {
    pub root: PathBuf,
    pub out: String,
    pub names: Names,
    cache: HashMap<String, Lift>,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub date: String,
    pub author: String,
    pub subject: String,
}

impl Vcs {
    pub fn open(root: &Path, out: &str, names: Names) -> Result<Vcs> {
        let top = git(root, &["rev-parse", "--show-toplevel"]).context("flow keeps history in git, and this folder is not a git repository. Run `git init` first")?;
        let top = PathBuf::from(top.trim());
        let root = std::fs::canonicalize(root)?;
        if top != root {
            bail!("run this from the repository root ({})", top.display());
        }
        Ok(Vcs { root, out: out.to_string(), names, cache: HashMap::new() })
    }

    pub fn head(&self) -> Option<String> {
        git(&self.root, &["rev-parse", "--verify", "-q", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn resolve(&self, rev: &str) -> Result<String> {
        Ok(git(&self.root, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")]).with_context(|| format!("no commit or branch called {rev}"))?.trim().to_string())
    }

    /// Lift many snapshots at once, spread across the machine's cores.
    pub fn prefetch(&mut self, revs: &[String]) -> Result<()> {
        let todo: Vec<String> = revs.iter().filter(|r| !self.cache.contains_key(*r)).cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        if todo.is_empty() {
            return Ok(());
        }
        let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
        let queue = std::sync::Mutex::new(todo);
        let done = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..jobs {
                s.spawn(|| loop {
                    let Some(rev) = queue.lock().unwrap().pop() else { break };
                    let l = lift_source(&Source::Rev { root: self.root.clone(), rev: rev.clone() }, &self.out);
                    done.lock().unwrap().push((rev, l));
                });
            }
        });
        // In a fixed order, so the same failure is reported however the threads were scheduled.
        let mut done = done.into_inner().unwrap();
        done.sort_by(|a, b| a.0.cmp(&b.0));
        for (rev, l) in done {
            self.cache.insert(rev, l?);
        }
        Ok(())
    }

    /// A snapshot already lifted by `lift_rev`.
    pub fn cached(&self, rev: &str) -> Option<&Lift> {
        self.cache.get(rev)
    }

    pub fn lift_rev(&mut self, rev: &str) -> Result<&Lift> {
        if !self.cache.contains_key(rev) {
            let l = lift_source(&Source::Rev { root: self.root.clone(), rev: rev.to_string() }, &self.out)?;
            self.cache.insert(rev.to_string(), l);
        }
        Ok(&self.cache[rev])
    }

    fn is_tracked_output(&self, p: &str) -> bool {
        p.starts_with(&format!("{}/", self.out)) || p == "flow.names.json" || p == crate::ids::FILE || p == crate::journeys::FILE || p == crate::titles::FILE
    }

    /// Changed files that are not read as journeys (styles, config, content), named so nothing hides.
    pub fn other(&self, paths: Vec<String>) -> Vec<String> {
        let mut v: Vec<String> = paths.into_iter().filter(|p| !is_source(p) && !self.is_tracked_output(p) && !crate::source::skipped(p)).collect();
        v.sort();
        v.dedup();
        v
    }

    fn worktree_paths(&self) -> Result<Vec<String>> {
        let s = git(&self.root, &["status", "--porcelain", "-z", "--untracked-files=all"])?;
        let mut out = vec![];
        let mut it = s.split('\0').filter(|x| !x.is_empty());
        while let Some(e) = it.next() {
            if e.len() < 4 {
                continue;
            }
            out.push(e[3..].to_string());
            if e.starts_with('R') || e.starts_with('C') {
                it.next();
            }
        }
        Ok(out)
    }

    /// Files that differ between a commit and the working folder, new ones included.
    pub fn changed_since(&self, rev: &str) -> Result<Vec<String>> {
        let mut paths = self.worktree_paths()?;
        paths.extend(git(&self.root, &["diff", "--name-only", "--no-renames", "-z", rev])?.split('\0').filter(|p| !p.is_empty()).map(str::to_string));
        Ok(self.other(paths))
    }

    /// The working folder, with any rename the code proves against the last saved snapshot.
    pub fn now(&mut self) -> Result<(Lift, Found)> {
        let mut now = lift_source(&Source::Dir(self.root.clone()), &self.out)?;
        let mut found = Found::default();
        // A merge only combines what each side recorded. HEAD is whichever side was merged into,
        // so comparing with it would make the result depend on the direction of the merge.
        let merging = self.merge_state_path().exists() || self.git_path("MERGE_HEAD").exists();
        if let Some(head) = self.head().filter(|_| !merging) {
            if !self.worktree_paths()?.is_empty() {
                found = crate::ids::reconcile(self.lift_rev(&head)?, &mut now);
            }
        }
        Ok((now, found))
    }

    /// Write down what `now` found. Nothing is written for a repo that has renamed nothing.
    pub fn record_ids(&self, now: &Lift) -> Result<()> {
        let p = self.root.join(crate::ids::FILE);
        let text = now.ids.render();
        if text != std::fs::read_to_string(&p).unwrap_or_default() {
            std::fs::write(p, text)?;
        }
        Ok(())
    }

    /// The working folder against the last saved snapshot.
    pub fn status(&mut self) -> Result<Report> {
        let (now, _) = self.now()?;
        let others = self.other(self.worktree_paths()?);
        let Some(head) = self.head() else {
            let empty = lift_source(&Source::Mem { root: self.root.clone(), files: Default::default() }, &self.out)?;
            return Ok(change::compare(&empty, &now, &self.names, others));
        };
        let names = self.names.clone();
        let before = self.lift_rev(&head)?;
        Ok(change::compare(before, &now, &names, others))
    }

    /// The change one commit made, against its first parent.
    pub fn change_of(&mut self, c: &Commit) -> Result<Report> {
        let names = self.names.clone();
        let others = match c.parents.first() {
            Some(p) => self.other(crate::source::changed_paths(&self.root, p, &c.id)?),
            None => self.other(crate::source::all_paths(&self.root, &c.id)?),
        };
        let after = c.id.clone();
        self.lift_rev(&after)?;
        let before_rev = c.parents.first().cloned();
        if let Some(p) = &before_rev {
            self.lift_rev(p)?;
        }
        let b = &self.cache[&after];
        Ok(match before_rev {
            Some(p) => change::compare(&self.cache[&p], b, &names, others),
            None => {
                let empty = lift_source(&Source::Mem { root: self.root.clone(), files: Default::default() }, &self.out)?;
                change::compare(&empty, b, &names, others)
            }
        })
    }

    /// Commits plus their first parents, lifted in parallel up front.
    pub fn commits_lifted(&mut self, n: usize, rev: &str) -> Result<Vec<Commit>> {
        let cs = self.commits(n, rev)?;
        let mut revs: Vec<String> = cs.iter().map(|c| c.id.clone()).collect();
        revs.extend(cs.iter().filter_map(|c| c.parents.first().cloned()));
        self.prefetch(&revs)?;
        Ok(cs)
    }

    pub fn commits(&self, n: usize, rev: &str) -> Result<Vec<Commit>> {
        self.commits_from(0, n, rev)
    }

    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    /// Let go of lifted snapshots, for something that runs for days.
    pub fn forget(&mut self) {
        self.cache.clear();
    }

    pub fn commits_from(&self, skip: usize, n: usize, rev: &str) -> Result<Vec<Commit>> {
        let s = git(&self.root, &["log", &format!("--skip={skip}"), &format!("-n{n}"), "--date=short", "--topo-order", "--format=%H%x1f%P%x1f%ad%x1f%an%x1f%s%x1e", rev])?;
        Ok(s.split('\x1e')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|rec| {
                let f: Vec<&str> = rec.split('\x1f').collect();
                Commit {
                    id: f[0].to_string(),
                    parents: f.get(1).map(|p| p.split_whitespace().map(str::to_string).collect()).unwrap_or_default(),
                    date: f.get(2).unwrap_or(&"").to_string(),
                    author: f.get(3).unwrap_or(&"").to_string(),
                    subject: f.get(4).unwrap_or(&"").to_string(),
                }
            })
            .collect())
    }

    /// A file inside git's own folder. In a linked worktree, `.git` is a file, not a folder.
    pub fn git_path(&self, name: &str) -> PathBuf {
        let p = git(&self.root, &["rev-parse", "--git-path", name]).map(|s| s.trim().to_string()).unwrap_or_else(|_| format!(".git/{name}"));
        self.root.join(p)
    }

    pub fn merge_state_path(&self) -> PathBuf {
        self.git_path("flow-merge.json")
    }

    /// Record the working folder as a new snapshot. Git does the storing; the message says what
    /// changed in journeys, so the history reads as product changes.
    pub fn save(&mut self, message: &str) -> Result<(String, Report)> {
        let ms = self.merge_state_path();
        if ms.exists() {
            crate::merge::refresh(self)?;
            let st: crate::merge::MergeState = serde_json::from_str(&std::fs::read_to_string(&ms)?)?;
            let open: Vec<String> = st.collisions.iter().filter(|c| !c.resolved).map(|c| c.label.clone()).collect();
            if !open.is_empty() {
                bail!("the merge still has {} collision(s) to decide: {}. Use `flow resolve <step> ours|theirs`", open.len(), open.join(", "));
            }
            let waiting: Vec<String> = st.checks.iter().filter(|c| !c.clean() && !st.accepted.contains(&c.scenario)).map(|c| c.scenario.clone()).collect();
            if !waiting.is_empty() {
                bail!("running the merge gave results nobody has accepted: {}. Change the code, or `flow accept <scenario>`", waiting.join(", "));
            }
        }
        let report = self.status()?;
        // A change no journey shows, such as a file mode, is still a change to record.
        if report.is_empty() && !self.git_path("MERGE_HEAD").exists() && self.worktree_paths()?.is_empty() {
            bail!("nothing to save: no journey or file changed");
        }
        let (l, _) = self.now()?;
        self.record_ids(&l)?;
        if self.root.join(&self.out).is_dir() {
            crate::write(&self.root.join(&self.out), &l.out.files)?;
        }
        git(&self.root, &["add", "-A"])?;
        let body = format!("{message}\n\n{}\nFlow-Journeys: {}\n", change::compact(&report).trim_end(), change::one_line(&report));
        let mut child = Command::new("git").arg("-C").arg(&self.root).args(["commit", "-q", "-F", "-"]).stdin(Stdio::piped()).spawn()?;
        child.stdin.take().unwrap().write_all(body.as_bytes())?;
        if !child.wait()?.success() {
            bail!("git could not save the snapshot");
        }
        let _ = std::fs::remove_file(ms);
        // What the agent was asked belongs to the change just saved.
        let _ = std::fs::remove_file(self.git_path("flow-asked"));
        let _ = std::fs::remove_file(self.git_path("flow-turn"));
        Ok((self.head().unwrap_or_default(), report))
    }
}
