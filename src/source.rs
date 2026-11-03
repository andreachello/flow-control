//! Where a repo's files come from: the working folder, a git snapshot, or memory (a merge preview).
//! Git is only a store of snapshots here. Flow asks it for snapshots and for lists of changed
//! file names, never for line diffs.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use walkdir::WalkDir;

const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "out", "coverage"];

#[derive(Clone)]
pub enum Source {
    Dir(PathBuf),
    Rev { root: PathBuf, rev: String },
    Mem { root: PathBuf, files: BTreeMap<String, String> },
}

/// Folders that never hold product code: dependencies, build output, hidden folders.
pub fn skipped(rel: &str) -> bool {
    let mut segs: Vec<&str> = rel.split('/').collect();
    segs.pop();
    segs.iter().any(|s| s.starts_with('.') || SKIP_DIRS.contains(s))
}

static TREES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<BTreeMap<String, String>>>>> = std::sync::OnceLock::new();
static BLOBS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>> = std::sync::OnceLock::new();

/// path -> file id for every file in a commit, read once per commit.
fn tree(root: &Path, rev: &str) -> Result<std::sync::Arc<BTreeMap<String, String>>> {
    let key = format!("{}\0{rev}", root.display());
    let cache = TREES.get_or_init(Default::default);
    if let Some(t) = cache.lock().unwrap().get(&key) {
        return Ok(t.clone());
    }
    let s = git(root, &["ls-tree", "-r", "-z", rev])?;
    let t: BTreeMap<String, String> = s
        .split('\0')
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut m = meta.split_whitespace();
            let kind = m.nth(1)?;
            // A link is not a file: the working folder skips links, so a snapshot must too.
            (kind == "blob" && !meta.starts_with("120000")).then(|| (path.to_string(), m.next().unwrap_or("").to_string()))
        })
        .collect();
    let t = std::sync::Arc::new(t);
    // Only cache full commit ids: names like HEAD move.
    if rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        cache.lock().unwrap().insert(key, t.clone());
    }
    Ok(t)
}

/// Files whose content differs between two commits, from their file ids. No diff is computed.
pub fn changed_paths(root: &Path, a: &str, b: &str) -> Result<Vec<String>> {
    let (ta, tb) = (tree(root, a)?, tree(root, b)?);
    let mut v: Vec<String> = ta.keys().chain(tb.keys()).filter(|p| ta.get(*p) != tb.get(*p)).cloned().collect();
    v.sort();
    v.dedup();
    Ok(v)
}

pub fn all_paths(root: &Path, rev: &str) -> Result<Vec<String>> {
    Ok(tree(root, rev)?.keys().cloned().collect())
}

/// Read many file versions in one git process.
fn cat_blobs(root: &Path, ids: &[String]) -> Result<Vec<(String, Option<String>)>> {
    let mut child = Command::new("git").arg("-C").arg(root).args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().context("could not run git")?;
    let mut stdin = child.stdin.take().unwrap();
    let req: String = ids.iter().map(|id| format!("{id}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(req.as_bytes()));
    let mut r = BufReader::new(child.stdout.take().unwrap());
    let mut out = vec![];
    for id in ids {
        let mut header = String::new();
        r.read_line(&mut header)?;
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.get(1) == Some(&"missing") || parts.len() < 3 {
            out.push((id.clone(), None));
            continue;
        }
        let size: usize = parts[2].parse().map_err(|_| anyhow!("bad git header {header:?}"))?;
        let mut buf = vec![0u8; size + 1];
        r.read_exact(&mut buf)?;
        buf.pop();
        out.push((id.clone(), String::from_utf8(buf).ok()));
    }
    writer.join().map_err(|_| anyhow!("git writer panicked"))??;
    child.wait()?;
    Ok(out)
}

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().context("could not run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// In a git repository, exactly the files a save would record: tracked, plus new files that are not
/// ignored. Ignored files stay out, so two clones of one commit lift to the same bytes. None for a
/// folder git does not manage, which is walked instead.
fn saved_files(root: &Path) -> Option<Vec<String>> {
    let ignored = Command::new("git").arg("-C").arg(root).args(["check-ignore", "-q", "."]).stderr(Stdio::null()).status().ok()?;
    if ignored.success() {
        return None;
    }
    let s = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"]).ok()?;
    let mut v: Vec<String> = s.split('\0').filter(|p| !p.is_empty() && std::fs::symlink_metadata(root.join(p)).map(|m| m.is_file()).unwrap_or(false)).map(str::to_string).collect();
    v.sort();
    v.dedup();
    Some(v)
}

/// Files git keeps with plain newlines but this folder holds with CRLF. A save would store them
/// without the CR, so they are read that way, and a folder lifts the same on every system.
fn line_ending_converted(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(listed) = git(root, &["ls-files", "-z", "--eol", "--cached", "--others", "--exclude-standard"]) else { return out };
    let auto = git(root, &["config", "--get", "core.autocrlf"]).map(|s| matches!(s.trim(), "true" | "input")).unwrap_or(false);
    for e in listed.split('\0') {
        let Some((meta, path)) = e.split_once('\t') else { continue };
        let m: Vec<&str> = meta.split_whitespace().collect();
        let (index, work, attr) = (m.first().copied().unwrap_or(""), m.get(1).copied().unwrap_or(""), m.get(2).copied().unwrap_or(""));
        let on = !attr.contains("-text") && (auto || attr.contains("text") || attr.contains("eol="));
        if on && matches!(index, "i/lf" | "i/") && matches!(work, "w/crlf" | "w/mixed") {
            out.insert(path.to_string());
        }
    }
    out
}

impl Source {
    pub fn root(&self) -> &Path {
        match self {
            Source::Dir(r) => r,
            Source::Rev { root, .. } | Source::Mem { root, .. } => root,
        }
    }

    /// Every file path in the snapshot, relative to the root, sorted.
    pub fn paths(&self) -> Result<Vec<String>> {
        let mut v = match self {
            Source::Dir(root) => {
                if let Some(v) = saved_files(root) {
                    return Ok(v.into_iter().filter(|p| !skipped(p)).collect());
                }
                let mut v = vec![];
                let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
                    e.depth() == 0 || !(e.file_type().is_dir() && { let n = e.file_name().to_string_lossy(); n.starts_with('.') || SKIP_DIRS.contains(&n.as_ref()) })
                });
                for e in walker {
                    let e = e?;
                    if e.file_type().is_file() {
                        v.push(e.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/"));
                    }
                }
                v
            }
            Source::Rev { root, rev } => tree(root, rev)?.keys().cloned().collect(),
            Source::Mem { files, .. } => files.keys().cloned().collect(),
        };
        v.retain(|p| !skipped(p));
        v.sort();
        Ok(v)
    }

    pub fn read(&self, p: &str) -> Option<String> {
        self.read_many(&[p.to_string()]).ok()?.pop()?.1
    }

    pub fn read_many(&self, paths: &[String]) -> Result<Vec<(String, Option<String>)>> {
        match self {
            Source::Dir(root) => {
                let crlf = line_ending_converted(root);
                Ok(paths
                    .iter()
                    .map(|p| {
                        let text = std::fs::read_to_string(root.join(p)).ok();
                        (p.clone(), if crlf.contains(p) { text.map(|t| t.replace("\r\n", "\n")) } else { text })
                    })
                    .collect())
            }
            Source::Mem { files, .. } => Ok(paths.iter().map(|p| (p.clone(), files.get(p).cloned())).collect()),
            Source::Rev { root, rev } => {
                // Every version of a file has an id; read each id from git once and reuse it.
                let ids = tree(root, rev)?;
                let cache = BLOBS.get_or_init(Default::default);
                let want: Vec<String> = {
                    let c = cache.lock().unwrap();
                    let mut w: Vec<String> = paths.iter().filter_map(|p| ids.get(p)).filter(|id| !c.contains_key(*id)).cloned().collect();
                    w.sort();
                    w.dedup();
                    w
                };
                if !want.is_empty() {
                    let got = cat_blobs(root, &want)?;
                    let mut c = cache.lock().unwrap();
                    if c.len() > 50_000 {
                        c.clear();
                    }
                    c.extend(got);
                }
                let c = cache.lock().unwrap();
                Ok(paths.iter().map(|p| (p.clone(), ids.get(p).and_then(|id| c.get(id).cloned().flatten()))).collect())
            }
        }
    }
}
