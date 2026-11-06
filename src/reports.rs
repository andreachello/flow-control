//! What each change did, kept on disk. Working it out reads the whole project twice, and a commit
//! never changes, so it is only ever worked out once. This holds what the code says and nothing
//! about journeys, which are applied when it is read, so regrouping a project keeps all of it.
//!
//! It lives outside the repo, because serving writes nothing there, and belongs to one version of
//! the code that reads changes, because a new reader can read a commit differently. Changing the
//! pages or the server keeps it. `build.rs` draws that line.

use crate::change::Report;
use crate::util::fnv64;
use crate::vcs::{Commit, Vcs};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// As many changes as the pages show first, read ahead so nobody waits for them.
const READ_AHEAD: usize = 150;
/// Each batch shares its lifts, and lets go of them after.
const BATCH: usize = 8;
const LOOK_AGAIN: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Store {
    dir: Option<PathBuf>,
}

fn base() -> Option<PathBuf> {
    std::env::var_os("FLOW_CACHE").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".flow/cache")))
}

impl Store {
    /// Without anywhere to write, this keeps nothing and everything still works.
    pub fn open(root: &Path) -> Store {
        let Some(base) = base() else { return Store { dir: None } };
        let project = base.join(fnv64(&root.display().to_string()).to_string());
        let names = std::fs::read_to_string(root.join("flow.names.json")).unwrap_or_default();
        let dir = project.join(fnv64(&format!("{}\0{names}", env!("FLOW_READER"))).to_string());
        // What older builds wrote will never be read again.
        for old in std::fs::read_dir(&project).into_iter().flatten().flatten() {
            if old.path() != dir {
                let _ = std::fs::remove_dir_all(old.path());
            }
        }
        Store { dir: std::fs::create_dir_all(&dir).ok().map(|_| dir) }
    }

    fn file(&self, sha: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{sha}.json")))
    }

    pub fn get(&self, sha: &str) -> Option<Report> {
        serde_json::from_slice(&std::fs::read(self.file(sha)?).ok()?).ok()
    }

    pub fn has(&self, sha: &str) -> bool {
        self.file(sha).is_some_and(|f| f.exists())
    }

    /// Whole or not at all, so a read can never see half of one.
    pub fn put(&self, sha: &str, r: &Report) {
        let (Some(file), Ok(text)) = (self.file(sha), serde_json::to_vec(r)) else { return };
        let part = file.with_extension(format!("{:?}.part", std::thread::current().id()));
        if std::fs::write(&part, text).is_ok() {
            let _ = std::fs::rename(&part, &file);
        }
    }
}

/// Work out one batch of changes, lifted together across the machine's cores.
pub fn read_batch(v: &mut Vcs, store: &Store, batch: &[Commit]) -> Result<()> {
    let mut revs: Vec<String> = batch.iter().map(|c| c.id.clone()).collect();
    revs.extend(batch.iter().filter_map(|c| c.parents.first().cloned()));
    v.prefetch(&revs)?;
    for c in batch {
        store.put(&c.id, &v.change_of(c)?);
    }
    Ok(())
}

/// Keep the latest changes read, on its own copy of the repo so no page ever waits behind it.
pub fn read_ahead(mut v: Vcs, store: Store) {
    let mut seen = None;
    loop {
        let head = v.head();
        if head != seen {
            let list = head.as_ref().and_then(|h| v.commits(READ_AHEAD, h).ok()).unwrap_or_default();
            let todo: Vec<Commit> = list.into_iter().filter(|c| !store.has(&c.id)).collect();
            for batch in todo.chunks(BATCH) {
                // One unreadable commit is left for the page to report, and does not stop the rest.
                let _ = read_batch(&mut v, &store, batch);
                v.forget();
            }
            seen = head;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_comes_back_as_it_went_in_and_a_broken_file_reads_as_missing() {
        let dir = std::env::temp_dir().join(format!("flow-reports-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Store { dir: Some(dir.clone()) };
        assert!(s.get("abc").is_none());
        s.put("abc", &Report { untouched: 7, ..Report::default() });
        assert!(s.has("abc"));
        assert_eq!(s.get("abc").unwrap().untouched, 7);
        std::fs::write(dir.join("abc.json"), "{half").unwrap();
        assert!(s.get("abc").is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn with_nowhere_to_write_nothing_is_kept_and_nothing_breaks() {
        let s = Store { dir: None };
        s.put("abc", &Report::default());
        assert!(!s.has("abc") && s.get("abc").is_none());
    }
}
