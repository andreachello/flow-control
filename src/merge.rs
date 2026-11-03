//! Merging by function, not by line.
//!
//! A file is a list of units: whole functions (slices) and the loose lines around them
//! (imports, setup). For each function, if only one side changed it, that side wins; if both
//! changed it differently, that is a collision, reported in the journeys that run it. Loose
//! lines merge as a set: a line either side added is kept, a line either side removed goes.
//! After merging, every journey that runs a function changed by one side and a different
//! function changed by the other is listed: the changes meet there, and a clean merge of
//! text says nothing about whether they agree.

use crate::change::journey_title;
use crate::names::Names;
use crate::parse::{parse_any as parse_file, Owner, ParsedFile, SUPPORT};
use crate::universal::readable as is_source;
use crate::source::{git, Source};
use crate::util::human;
use crate::vcs::Vcs;
use crate::Lift;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::process::Stdio;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Collision {
    pub file: String,
    /// The function's name, or empty for a whole file that is not code.
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub journeys: Vec<String>,
    pub resolved: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MergeState {
    pub theirs: String,
    pub theirs_name: String,
    pub written: Vec<String>,
    pub collisions: Vec<Collision>,
    /// Scenario results that need a person's decision before the merge can be saved.
    #[serde(default)]
    pub checks: Vec<crate::scenario::Check>,
    #[serde(default)]
    pub accepted: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub base: String,
    pub ours: String,
    pub theirs: String,
    /// The merged tree: the whole answer, comparable across machines.
    pub tree: String,
    /// Every branch being merged in, by commit id.
    pub heads: Vec<String>,
    /// The branch that collided, when several were merged at once. Nothing is written then.
    pub blocked: Option<String>,
    /// Merged text for paths that needed merging. None means the file is removed.
    pub files: BTreeMap<String, Option<String>>,
    /// Paths taken whole, as (mode, object id). Anything can pass through here: binaries, links.
    pub whole: BTreeMap<String, (String, String)>,
    /// File modes for merged text, where both sides changed the file.
    pub modes: BTreeMap<String, String>,
    pub taken: Vec<(String, String)>,
    pub collisions: Vec<Collision>,
    /// (journey, functions changed here, functions changed there)
    pub meets: Vec<(String, Vec<String>, Vec<String>)>,
    pub already_in: bool,
    /// Nothing changed here since the branch split off, so the branch is simply taken whole.
    pub fast_forward: bool,
    /// (file, function) changed on both sides and combined because the edits did not overlap.
    pub combined: Vec<(String, String)>,
    /// (function, from file, to file): moved on one side, merged with the other side's edits.
    pub moved: Vec<(String, String, String)>,
    /// Files that are not code, changed on both sides, merged as text.
    pub text_merged: Vec<String>,
    /// Ids of the journeys in `meets`, in the same order.
    pub meet_ids: Vec<String>,
}

#[derive(Clone, PartialEq)]
enum Unit {
    /// A whole function or declaration: name, text, behaviour fingerprint.
    Code(String, String, String),
    Line(String),
}

/// A part may be split around others (a class's header and closing brace, around its methods).
/// Each contiguous run is its own unit, so the file always prints back in order.
fn units(pf: &ParsedFile) -> Vec<Unit> {
    let mut out = vec![];
    let mut runs: HashMap<usize, usize> = HashMap::new();
    let mut r = 0;
    while r < pf.lines.len() {
        match pf.owner[r] {
            Owner::Slice(i) if i != SUPPORT => {
                let start = r;
                while r < pf.lines.len() && pf.owner[r] == Owner::Slice(i) {
                    r += 1;
                }
                let k = runs.entry(i).or_insert(0);
                let s = &pf.slices[i];
                let text = pf.lines[start..r].join("\n");
                let whole = r - start == s.lines.len();
                let name = if *k == 0 { s.name.clone() } else { format!("{}#{}", s.name, k) };
                let shape = if whole { s.shape.clone() } else { crate::util::fnv64(&text.split_whitespace().collect::<String>()) };
                out.push(Unit::Code(name, text, shape));
                *k += 1;
            }
            _ => {
                out.push(Unit::Line(pf.lines[r].clone()));
                r += 1;
            }
        }
    }
    out
}

fn code_map(u: &[Unit]) -> BTreeMap<String, (String, String)> {
    u.iter().filter_map(|x| if let Unit::Code(n, t, s) = x { Some((n.clone(), (t.clone(), s.clone()))) } else { None }).collect()
}

fn line_set(u: &[Unit]) -> BTreeSet<String> {
    u.iter().filter_map(|x| if let Unit::Line(t) = x { (!t.trim().is_empty()).then(|| t.clone()) } else { None }).collect()
}

fn same_key(a: &Unit, b: &Unit) -> bool {
    match (a, b) {
        (Unit::Code(x, ..), Unit::Code(y, ..)) => x == y,
        (Unit::Line(x), Unit::Line(y)) => x == y,
        _ => false,
    }
}

static TMP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Git's own three-way text merge, used only inside one function or one non-code file, and always
/// reported. None when the two sides edit the same lines.
pub fn text_merge(base: &str, ours: &str, theirs: &str) -> Result<Option<String>> {
    run_text_merge(base, ours, theirs, false)
}

/// `union`: keep both sides' lines instead of calling overlapping edits a collision.
fn run_text_merge(base: &str, ours: &str, theirs: &str, union: bool) -> Result<Option<String>> {
    // Fixed order, so the result cannot depend on which side is called ours.
    let (ours, theirs) = if ours <= theirs { (ours, theirs) } else { (theirs, ours) };
    // Git will not text-merge binary content: that is a decision for a person, not an error.
    if base.contains('\0') || ours.contains('\0') || theirs.contains('\0') {
        return Ok(None);
    }
    let dir = std::env::temp_dir().join(format!("flow-merge-{}-{}", std::process::id(), TMP.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    std::fs::create_dir_all(&dir)?;
    let nl = |t: &str| if t.ends_with('\n') { t.to_string() } else { format!("{t}\n") };
    for (name, t) in [("ours", ours), ("base", base), ("theirs", theirs)] {
        std::fs::write(dir.join(name), nl(t))?;
    }
    let out = std::process::Command::new("git").current_dir(&dir).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").args(["merge-file", "-p", "-q"]).args(union.then_some("--union")).args(["ours", "base", "theirs"]).output()?;
    let _ = std::fs::remove_dir_all(&dir);
    match out.status.code() {
        Some(0) => {
            let mut m = String::from_utf8_lossy(&out.stdout).to_string();
            // A final newline is merged like any other edit: the side that changed it wins.
            let (bn, on, tn) = (base.ends_with('\n'), ours.ends_with('\n'), theirs.ends_with('\n'));
            if !(if on == bn { tn } else { on }) && m.ends_with('\n') {
                m.pop();
            }
            Ok(Some(m))
        }
        // Git calls edits on neighbouring lines a conflict. When no lines were added or removed,
        // merge line by line instead: only the same line changed two ways is a real collision.
        Some(c) if (1..=127).contains(&c) => Ok(line_by_line(base, ours, theirs)),
        _ => bail!("git merge-file failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
    }
}

fn line_by_line(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let (b, o, t): (Vec<&str>, Vec<&str>, Vec<&str>) = (base.split('\n').collect(), ours.split('\n').collect(), theirs.split('\n').collect());
    if b.len() != o.len() || b.len() != t.len() {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    for i in 0..b.len() {
        out.push(if o[i] == t[i] || t[i] == b[i] {
            o[i]
        } else if o[i] == b[i] {
            t[i]
        } else {
            return None;
        });
    }
    Some(out.join("\n"))
}

/// The behaviour fingerprint of a lone function's text.
fn shape_of(path: &str, text: &str) -> String {
    parse_file(path, text, &[]).map(|p| p.slices.iter().skip(1).map(|s| s.shape.clone()).collect::<Vec<_>>().join(",")).unwrap_or_default()
}

pub enum FnMerge {
    /// Both sides mean the same; this text is the one to keep.
    Same(String),
    /// Only the first side changed it.
    First(String),
    /// Only the second side changed it.
    Second(String),
    Combined(String),
    Conflict,
}

/// Merge one function. Formatting-only differences never conflict; if both sides changed what it
/// does, their edits are combined line by line when they do not touch the same lines.
pub fn merge_fn(_path: &str, base: Option<(&str, &str)>, ours: (&str, &str), theirs: (&str, &str)) -> Result<FnMerge> {
    if ours.1 == theirs.1 {
        // The same behaviour. When one side only reformatted, the side that kept the old text
        // gives way, and the rule cannot depend on which side is called ours.
        let reformatted = base.map(|b| b.0 == ours.0).unwrap_or(false);
        return Ok(FnMerge::Same(if reformatted { theirs.0 } else { ours.0 }.to_string()));
    }
    if let Some((_, bs)) = base {
        if bs == ours.1 {
            return Ok(FnMerge::Second(theirs.0.to_string()));
        }
        if bs == theirs.1 {
            return Ok(FnMerge::First(ours.0.to_string()));
        }
    }
    Ok(match text_merge(base.map(|b| b.0).unwrap_or(""), ours.0, theirs.0)? {
        Some(m) => FnMerge::Combined(m),
        None => FnMerge::Conflict,
    })
}

#[derive(Debug, Clone)]
pub struct FnCollision {
    pub name: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

pub struct FileMerge {
    pub text: Option<String>,
    pub taken: Vec<String>,
    /// Functions both sides changed, combined because their edits did not overlap.
    pub combined: Vec<String>,
    pub collisions: Vec<FnCollision>,
}

/// Three-way merge of one source file, a function at a time.
pub fn merge_file(path: &str, base: Option<&str>, ours: &str, theirs: &str) -> Result<FileMerge> {
    merge_file_with(path, base, ours, theirs, &BTreeSet::new(), &BTreeSet::new())
}

/// `poison`: functions with no trustworthy ancestor. `gone`: functions moved out of this file.
///
/// The two sides are put in a fixed order first, so A into B and B into A give the same text. Only
/// what a collision leaves on disk differs: our version, until someone decides.
pub fn merge_file_with(path: &str, base: Option<&str>, ours: &str, theirs: &str, poison: &BTreeSet<String>, gone: &BTreeSet<String>) -> Result<FileMerge> {
    let flip = ours > theirs;
    let (a, b) = if flip { (theirs, ours) } else { (ours, theirs) };
    let mut m = merge_sorted(path, base, a, b, flip, poison, gone)?;
    if flip {
        for c in m.collisions.iter_mut() {
            std::mem::swap(&mut c.ours, &mut c.theirs);
        }
    }
    let (taken_a, taken_b) = m.taken;
    Ok(FileMerge { text: m.text, taken: if flip { taken_a } else { taken_b }, combined: m.combined, collisions: m.collisions })
}

struct Sorted {
    text: Option<String>,
    /// What came from the first side, and from the second.
    taken: (Vec<String>, Vec<String>),
    combined: Vec<String>,
    collisions: Vec<FnCollision>,
}

/// `keep_b`: the second side is really ours, so its version is the one a collision leaves in place.
fn merge_sorted(path: &str, base: Option<&str>, ours: &str, theirs: &str, keep_b: bool, poison: &BTreeSet<String>, gone: &BTreeSet<String>) -> Result<Sorted> {
    let (pb, po, pt) = (parse_file(path, base.unwrap_or(""), &[])?, parse_file(path, ours, &[])?, parse_file(path, theirs, &[])?);
    let kept = |u: Vec<Unit>| -> Vec<Unit> { u.into_iter().filter(|x| !matches!(x, Unit::Code(n, ..) if gone.contains(n))).collect() };
    let (ub, uo, ut) = (kept(if base.is_some() { units(&pb) } else { vec![] }), kept(units(&po)), kept(units(&pt)));
    let (cb, co, ct) = (code_map(&ub), code_map(&uo), code_map(&ut));
    let (lb, lt, lo) = (line_set(&ub), line_set(&ut), line_set(&uo));
    let mut out: Vec<Unit> = vec![];
    let (mut taken_a, mut taken_b, mut combined, mut collisions) = (vec![], vec![], vec![], vec![]);
    for u in &uo {
        match u {
            Unit::Line(t) => {
                if !t.trim().is_empty() && lb.contains(t) && !lt.contains(t) {
                    continue; // they removed this line
                }
                out.push(u.clone());
            }
            Unit::Code(n, o, os) => {
                let b = cb.get(n).filter(|_| !poison.contains(n));
                match ct.get(n) {
                    Some((t, ts)) => match merge_fn(path, b.map(|(x, y)| (x.as_str(), y.as_str())), (o, os), (t, ts))? {
                        FnMerge::Same(x) => out.push(Unit::Code(n.clone(), x, os.clone())),
                        FnMerge::First(x) => {
                            out.push(Unit::Code(n.clone(), x, os.clone()));
                            taken_a.push(n.clone());
                        }
                        FnMerge::Second(x) => {
                            out.push(Unit::Code(n.clone(), x, ts.clone()));
                            taken_b.push(n.clone());
                        }
                        FnMerge::Combined(x) => {
                            out.push(Unit::Code(n.clone(), x, String::new()));
                            combined.push(n.clone());
                        }
                        FnMerge::Conflict => {
                            collisions.push(FnCollision { name: n.clone(), base: b.map(|x| x.0.clone()), ours: Some(o.clone()), theirs: Some(t.clone()) });
                            out.push(if keep_b { Unit::Code(n.clone(), t.clone(), ts.clone()) } else { u.clone() });
                        }
                    },
                    None if poison.contains(n) => {
                        collisions.push(FnCollision { name: n.clone(), base: cb.get(n).map(|x| x.0.clone()), ours: Some(o.clone()), theirs: None });
                        if !keep_b {
                            out.push(u.clone());
                        }
                    }
                    None => match b {
                        Some((_, bs)) if bs == os => taken_b.push(format!("{n} (removed)")),
                        Some((bt, _)) => {
                            collisions.push(FnCollision { name: n.clone(), base: Some(bt.clone()), ours: Some(o.clone()), theirs: None });
                            if !keep_b {
                                out.push(u.clone());
                            }
                        }
                        None => {
                            out.push(u.clone());
                            taken_a.push(n.clone());
                        }
                    },
                }
            }
        }
    }
    // What only they added goes after the unit that precedes it on their side.
    for (i, u) in ut.iter().enumerate() {
        let add = match u {
            Unit::Code(n, t, ts) => {
                if co.contains_key(n) {
                    false
                } else if poison.contains(n) {
                    collisions.push(FnCollision { name: n.clone(), base: cb.get(n).map(|x| x.0.clone()), ours: None, theirs: Some(t.clone()) });
                    keep_b
                } else if let Some((bt, bs)) = cb.get(n) {
                    if bs != ts {
                        collisions.push(FnCollision { name: n.clone(), base: Some(bt.clone()), ours: None, theirs: Some(t.clone()) });
                        keep_b
                    } else {
                        taken_a.push(format!("{n} (removed)"));
                        false
                    }
                } else {
                    taken_b.push(n.clone());
                    true
                }
            }
            Unit::Line(t) => !t.trim().is_empty() && !lb.contains(t) && !lo.contains(t),
        };
        if !add {
            continue;
        }
        let pos = ut[..i].iter().rev().find_map(|p| out.iter().position(|x| same_key(x, p))).map(|p| p + 1).unwrap_or(0);
        out.insert(pos, u.clone());
    }
    let text = out.iter().map(|u| match u {
        Unit::Code(_, t, _) => t.as_str(),
        Unit::Line(t) => t.as_str(),
    });
    Ok(Sorted { text: Some(text.collect::<Vec<_>>().join("\n")), taken: (taken_a, taken_b), combined, collisions })
}

fn sid_set(l: &Lift) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (fi, f) in l.repo.files.iter().enumerate() {
        for si in 1..f.slices.len() {
            m.insert(l.repo.sid((fi, si)), f.slices[si].shape.clone());
        }
    }
    m
}

fn label_of(names: &Names, sid: &str) -> String {
    names.steps.get(sid).map(|n| n.name.clone()).unwrap_or_else(|| human(sid.rsplit('#').next().unwrap_or(sid)))
}

/// One tree entry: (file mode, git object id).
type Entry = (String, String);

/// (file, function) pairs a virtual base could not settle. Empty function means the whole file.
/// Keyed by virtual commit. Those parts have no trustworthy ancestor, so a later merge may not
/// treat either side as "unchanged" there, the same job git's conflict markers do in a virtual base.
type Poison = BTreeMap<String, BTreeSet<(String, String)>>;

fn blobs(root: &std::path::Path, rev: &str) -> Result<BTreeMap<String, Entry>> {
    let s = git(root, &["ls-tree", "-r", "-z", rev])?;
    Ok(s.split('\0')
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut m = meta.split_whitespace();
            let mode = m.next()?.to_string();
            Some((path.to_string(), (mode, m.nth(1)?.to_string())))
        })
        .collect())
}

/// Run git with extra environment and optional stdin.
fn run(root: &std::path::Path, args: &[&str], env: &[(&str, &str)], input: Option<&str>) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(root).args(args).envs(env.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let body = input.unwrap_or("").to_string();
    let writer = std::thread::spawn(move || stdin.write_all(body.as_bytes()));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The merge result as a git tree: `ours` with this plan's changes on top. The tree is the merge's
/// whole answer, so its id is what two machines compare.
fn write_tree(v: &Vcs, ours: &str, p: &Plan) -> Result<String> {
    let root = &v.root;
    let bo = blobs(root, ours)?;
    let mut entries: BTreeMap<String, Option<Entry>> = p.whole.iter().map(|(k, e)| (k.clone(), Some(e.clone()))).collect();
    for (path, val) in &p.files {
        let e = match val {
            Some(text) => {
                let oid = run(root, &["hash-object", "-w", "--stdin"], &[], Some(text))?.trim().to_string();
                let mode = p.modes.get(path).or(p.whole.get(path).map(|e| &e.0)).or(bo.get(path).map(|e| &e.0)).cloned().unwrap_or_else(|| "100644".to_string());
                Some((mode, oid))
            }
            None => None,
        };
        entries.insert(path.clone(), e);
    }
    let index = std::env::temp_dir().join(format!("flow-index-{}-{}", std::process::id(), TMP.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let path = index.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", path.as_str())];
    let tree = (|| -> Result<String> {
        run(root, &["read-tree", ours], &env, None)?;
        let oid_len = run(root, &["rev-parse", ours], &[], None)?.trim().len();
        let none = "0".repeat(oid_len);
        let mut info = String::new();
        for (path, e) in &entries {
            match e {
                Some((mode, oid)) => info.push_str(&format!("{mode} {oid}\t{path}\0")),
                None => info.push_str(&format!("0 {none}\t{path}\0")),
            }
        }
        run(root, &["update-index", "--add", "--replace", "-z", "--index-info"], &env, Some(&info))?;
        Ok(run(root, &["write-tree"], &env, None)?.trim().to_string())
    })();
    let _ = std::fs::remove_file(&index);
    tree
}

/// A commit object nothing points at, used as a stepping stone. Its author and date are fixed, so
/// the same inputs give the same id on every machine.
pub fn commit_tree(v: &Vcs, tree: &str, parents: &[&str]) -> Result<String> {
    let mut args = vec!["commit-tree", tree];
    for p in parents {
        args.extend(["-p", p]);
    }
    args.extend(["-m", "flow merge step"]);
    let d = "1000000000 +0000";
    let env = [("GIT_AUTHOR_NAME", "flow"), ("GIT_AUTHOR_EMAIL", "flow@invalid"), ("GIT_AUTHOR_DATE", d), ("GIT_COMMITTER_NAME", "flow"), ("GIT_COMMITTER_EMAIL", "flow@invalid"), ("GIT_COMMITTER_DATE", d)];
    Ok(run(&v.root, &args, &env, None)?.trim().to_string())
}

/// The ancestor to merge from. With several best ancestors (branches that merged each other), they
/// are merged together first, in id order, into one virtual ancestor, as git does.
fn merge_base(v: &mut Vcs, a: &str, b: &str, poison: &mut Poison) -> Result<String> {
    let found = git(&v.root, &["merge-base", "--all", a, b]).unwrap_or_default();
    let mut bases: Vec<String> = found.split_whitespace().map(str::to_string).collect();
    bases.sort();
    let mut it = bases.into_iter();
    let Some(mut cur) = it.next() else { return Ok(String::new()) };
    for next in it {
        cur = virtual_merge(v, &cur, &next, poison)?;
    }
    Ok(cur)
}

fn virtual_merge(v: &mut Vcs, a: &str, b: &str, poison: &mut Poison) -> Result<String> {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    let p = step(v, x, y, true, poison)?;
    let c = commit_tree(v, &p.tree, &[x, y])?;
    let mut bad: BTreeSet<(String, String)> = p.collisions.iter().map(|k| (k.file.clone(), k.name.clone())).collect();
    for k in [x, y, p.base.as_str()] {
        bad.extend(poison.get(k).into_iter().flatten().cloned());
    }
    poison.insert(c.clone(), bad);
    Ok(c)
}

fn collision(path: &str, base: Option<String>, ours: Option<String>, theirs: Option<String>) -> Collision {
    Collision { file: path.to_string(), name: String::new(), label: String::new(), base, ours, theirs, journeys: vec![], resolved: false }
}

/// Generated files are never merged: they are rebuilt from the merged code.
fn generated(out: &str, path: &str) -> bool {
    let Some(name) = path.strip_prefix(out).and_then(|p| p.strip_prefix('/')) else { return false };
    !name.contains('/') && (name.ends_with(".flow.yaml") || (name.starts_with('_') && name.ends_with(".yaml")))
}

/// What a commit's .gitattributes says about merging a path. Read from the commit, never the folder.
fn merge_attr(root: &std::path::Path, rev: &str, path: &str) -> Result<String> {
    let s = git(root, &["check-attr", "--source", rev, "merge", "--", path]).context("reading .gitattributes from a commit needs git 2.41 or newer")?;
    Ok(s.trim().rsplit(": ").next().unwrap_or("unspecified").to_string())
}

/// old path -> new path, for files a commit renamed since `base`.
fn renames(root: &std::path::Path, base: &str, rev: &str) -> Result<BTreeMap<String, String>> {
    let s = git(root, &["diff-tree", "-r", "-z", "-M50%", "-l0", "--name-status", "--diff-filter=R", base, rev])?;
    let f: Vec<&str> = s.split('\0').collect();
    Ok(f.chunks_exact(3).map(|c| (c[1].to_string(), c[2].to_string())).collect())
}

pub fn plan(v: &mut Vcs, theirs_rev: &str) -> Result<Plan> {
    plan_many(v, &[theirs_rev.to_string()])
}

/// Plan merging one or more branches into HEAD. Several branches are merged one after another in
/// commit id order, so the order they were named in cannot change the result. Like git's octopus
/// merge, this needs no decisions from anyone: a collision blocks the whole merge.
pub fn plan_many(v: &mut Vcs, revs: &[String]) -> Result<Plan> {
    let Some(ours) = v.head() else { bail!("save a first snapshot before merging") };
    let mut args = vec!["merge-base".to_string(), "--independent".to_string(), ours.clone()];
    for r in revs {
        args.push(v.resolve(r)?);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut others: Vec<String> = git(&v.root, &refs)?.split_whitespace().filter(|h| *h != ours).map(str::to_string).collect();
    others.sort();
    let mut poison = Poison::new();
    let Some(first) = others.first().cloned() else {
        let theirs = args.last().cloned().unwrap_or_default();
        return Ok(Plan { base: theirs.clone(), ours, theirs, already_in: true, ..Default::default() });
    };
    if others.len() == 1 {
        let mut p = step(v, &ours, &first, false, &mut poison)?;
        p.heads = others;
        return Ok(p);
    }
    let mut total = Plan { ours: ours.clone(), theirs: first, heads: others.clone(), ..Default::default() };
    let mut cur = ours.clone();
    for h in &others {
        let p = step(v, &cur, h, false, &mut poison)?;
        total.base = p.base.clone();
        total.taken.extend(p.taken);
        total.combined.extend(p.combined);
        total.moved.extend(p.moved);
        total.text_merged.extend(p.text_merged);
        total.meets.extend(p.meets);
        let blocked = !p.collisions.is_empty();
        total.collisions.extend(p.collisions);
        if blocked {
            total.blocked = Some(h.clone());
            return Ok(total);
        }
        cur = if p.fast_forward { h.clone() } else { commit_tree(v, &p.tree, &[&cur, h])? };
        total.tree = git(&v.root, &["rev-parse", &format!("{cur}^{{tree}}")])?.trim().to_string();
    }
    Ok(total)
}

/// One two-sided merge. The result depends only on the two commits and their history. With
/// `quiet`, it is a stepping stone: nothing is reported, and generated files are left alone.
fn step(v: &mut Vcs, ours: &str, theirs: &str, quiet: bool, poison: &mut Poison) -> Result<Plan> {
    let base = merge_base(v, ours, theirs, poison)?;
    let bad = poison.get(&base).cloned().unwrap_or_default();
    let tree_of = |v: &Vcs, r: &str| git(&v.root, &["rev-parse", &format!("{r}^{{tree}}")]).map(|s| s.trim().to_string());
    let mut p = Plan { base: base.clone(), ours: ours.to_string(), theirs: theirs.to_string(), heads: vec![theirs.to_string()], ..Default::default() };
    if base == theirs {
        p.already_in = true;
        p.tree = tree_of(v, ours)?;
        return Ok(p);
    }
    if base == ours {
        p.fast_forward = true;
        p.tree = tree_of(v, theirs)?;
        return Ok(p);
    }
    let root = v.root.clone();
    let out = v.out.trim_start_matches("./").trim_end_matches('/').to_string();
    let (bb, bo, bt) = (if base.is_empty() { BTreeMap::new() } else { blobs(&root, &base)? }, blobs(&root, ours)?, blobs(&root, theirs)?);
    let rev = |r: &str| Source::Rev { root: root.clone(), rev: r.to_string() };
    // A file with a NUL byte is binary, even when it happens to be valid UTF-8.
    let text_of = |r: &str, path: &str, present: bool| if present { rev(r).read(path).filter(|s| !s.contains('\0')) } else { None };
    let regular = |e: Option<&Entry>| e.map(|x| x.0.starts_with("100")).unwrap_or(true);
    let attrs = bo.keys().chain(bt.keys()).any(|k| k == ".gitattributes" || k.ends_with("/.gitattributes"));

    // A file one side renamed and the other edited: the edit follows the file to its new path. The
    // two sides stay in order, so this reads the same whichever one did the renaming.
    let mut followed: BTreeSet<String> = BTreeSet::new();
    if !base.is_empty() && bb.keys().any(|k| !bo.contains_key(k) || !bt.contains_key(k)) {
        let (ro, rt) = (renames(&root, &base, ours)?, renames(&root, &base, theirs)?);
        for (mover, editor, list, kept, mover_tree) in [(ours, theirs, &ro, &bt, &bo), (theirs, ours, &rt, &bo, &bt)] {
            for (old, new) in list {
                let (b, e) = (bb.get(old), kept.get(old));
                let (Some(e), true) = (e, e != b) else { continue };
                if generated(&out, new) || kept.contains_key(new) || bad.iter().any(|(f, _)| f == old || f == new) || !e.0.starts_with("100") {
                    continue;
                }
                let (Some(tb), Some(tm), Some(te)) = (text_of(&base, old, true), text_of(mover, new, true), text_of(editor, old, true)) else { continue };
                let (to, tt) = if mover == ours { (&tm, &te) } else { (&te, &tm) };
                let merged = if is_source(new) && !crate::universal::is_plain(new) {
                    let m = merge_file_with(new, Some(&tb), to, tt, &BTreeSet::new(), &BTreeSet::new())?;
                    for c in m.collisions {
                        p.collisions.push(Collision { name: c.name, ..collision(new, c.base, c.ours, c.theirs) });
                    }
                    p.combined.extend(m.combined.into_iter().map(|n| (new.clone(), n)));
                    m.text
                } else {
                    text_merge(&tb, to, tt)?
                };
                let Some(text) = merged else { continue };
                p.files.insert(new.clone(), Some(text));
                if let Some(e) = mover_tree.get(new) {
                    p.modes.insert(new.clone(), e.0.clone());
                }
                if bo.contains_key(old) {
                    p.files.insert(old.clone(), None);
                }
                followed.extend([old.clone(), new.clone()]);
                p.moved.push((new.rsplit('/').next().unwrap_or(new).to_string(), old.clone(), new.clone()));
            }
        }
    }

    let paths: BTreeSet<&String> = bb.keys().chain(bo.keys()).chain(bt.keys()).collect();
    for path in paths {
        if generated(&out, path) || followed.contains(path) {
            continue;
        }
        let (b, o, t) = (bb.get(path), bo.get(path), bt.get(path));
        if bad.contains(&(path.clone(), String::new())) && o != t {
            p.collisions.push(collision(path, text_of(&base, path, b.is_some()), text_of(ours, path, o.is_some()), text_of(theirs, path, t.is_some())));
            continue;
        }
        if o == t {
            continue;
        }
        // A file holding something the ancestor could not settle cannot use the shortcuts below:
        // the ancestor may match one side only because it had to hold some version.
        let tainted = bad.iter().any(|(f, _)| f == path);
        if t == b && !tainted {
            continue; // nothing new from their side
        }
        if o == b && !tainted {
            p.taken.push((path.clone(), String::new()));
            match t {
                None => {
                    p.files.insert(path.clone(), None);
                }
                // By object id, never through a string, so every kind of file survives exactly.
                Some(e) => {
                    p.whole.insert(path.clone(), e.clone());
                }
            }
            continue;
        }
        // Both sides changed this path. Symbolic links and submodules cannot be combined.
        if !regular(b) || !regular(o) || !regular(t) {
            p.collisions.push(collision(path, None, None, None));
            continue;
        }
        let mut mode = None;
        if let (Some(eo), Some(et)) = (o, t) {
            let m = if b.map(|x| &x.0) == Some(&eo.0) { et.0.clone() } else { eo.0.clone() };
            // Content and mode merge separately: one side's chmod and the other's edit both stay.
            let bo_id = b.map(|x| &x.1);
            if eo.1 == et.1 || (bo_id == Some(&et.1) && !tainted) {
                p.whole.insert(path.clone(), (m, eo.1.clone()));
                continue;
            }
            if bo_id == Some(&eo.1) && !tainted {
                p.whole.insert(path.clone(), (m, et.1.clone()));
                continue;
            }
            mode = Some(m);
        }
        let (tb, to, tt) = (text_of(&base, path, b.is_some()), text_of(ours, path, o.is_some()), text_of(theirs, path, t.is_some()));
        // Only git's built-in rules count. A named driver lives in someone's personal config, and a
        // merge must not depend on that. If the two sides disagree, nothing is merged automatically.
        let rule = if attrs {
            let (a, b) = (merge_attr(&root, ours, path)?, merge_attr(&root, theirs, path)?);
            if a == b { a } else { "unset".to_string() }
        } else {
            "unspecified".to_string()
        };
        let union = rule == "union";
        let readable = (b.is_none() || tb.is_some()) && !matches!(rule.as_str(), "unset" | "binary");
        match (is_source(path) && !crate::universal::is_plain(path) && !union, to, tt) {
            (true, Some(to), Some(tt)) if readable => {
                let names: BTreeSet<String> = bad.iter().filter(|(f, _)| f == path).map(|(_, n)| n.clone()).collect();
                let m = merge_file_with(path, tb.as_deref(), &to, &tt, &names, &BTreeSet::new())?;
                for n in &m.taken {
                    p.taken.push((path.clone(), n.clone()));
                }
                for n in &m.combined {
                    p.combined.push((path.clone(), n.clone()));
                }
                for c in m.collisions {
                    p.collisions.push(Collision { name: c.name, ..collision(path, c.base, c.ours, c.theirs) });
                }
                p.files.insert(path.clone(), m.text);
            }
            (false, Some(to), Some(tt)) if readable => match run_text_merge(tb.as_deref().unwrap_or(""), &to, &tt, union)? {
                Some(m) => {
                    p.files.insert(path.clone(), Some(m));
                    p.text_merged.push(path.clone());
                }
                None => p.collisions.push(collision(path, tb, Some(to), Some(tt))),
            },
            (_, to, tt) => {
                p.collisions.push(collision(path, tb, to, tt));
                continue;
            }
        }
        if let Some(m) = mode {
            p.modes.insert(path.clone(), m);
        }
    }

    // A function one side moved to another file and the other side edited in place: follow it to
    // its new home and merge the edit there, instead of calling it a collision.
    let find_moved = |l: &Lift, base_l: Option<&Lift>, name: &str, from: &str| -> Option<(String, String)> {
        l.repo.files.iter().enumerate().find_map(|(fi, f)| {
            if f.path == from {
                return None;
            }
            let &si = f.by_name.get(name)?;
            let was_there = base_l.and_then(|b| b.repo.file_idx(&f.path)).map(|bi| base_l.unwrap().repo.files[bi].by_name.contains_key(name)).unwrap_or(false);
            (!was_there).then(|| (f.path.clone(), l.repo.slice((fi, si)).text(f)))
        })
    };
    let mut keep = vec![];
    let mut gone: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let collisions = std::mem::take(&mut p.collisions);
    for c in collisions {
        if c.name.is_empty() || c.ours.is_some() == c.theirs.is_some() || bad.contains(&(c.file.clone(), c.name.clone())) {
            keep.push(c);
            continue;
        }
        let ours_moved = c.ours.is_none();
        let mover_rev = if ours_moved { ours.to_string() } else { theirs.to_string() };
        if !base.is_empty() {
            v.lift_rev(&base)?;
        }
        v.lift_rev(&mover_rev)?;
        let found = find_moved(v.cached(&mover_rev).unwrap(), if base.is_empty() { None } else { v.cached(&base) }, &c.name, &c.file);
        let Some((to, moved_text)) = found else {
            keep.push(c);
            continue;
        };
        let edited = c.ours.clone().or(c.theirs.clone()).unwrap();
        let sh = |t: &str| shape_of(&to, t);
        // Moving keeps the body, so the edit is compared with the body as it was before the move.
        let merged = match &c.base {
            Some(b) if sh(b) == sh(&moved_text) => Some(edited.clone()),
            Some(b) if sh(b) == sh(&edited) => Some(moved_text.clone()),
            Some(b) => text_merge(b, &moved_text, &edited)?,
            None => None,
        };
        let Some(new_fn) = merged else {
            keep.push(c);
            continue;
        };
        let current_to = match p.files.get(&to) {
            Some(Some(t)) => t.clone(),
            _ => rev(&mover_rev).read(&to).unwrap_or_default(),
        };
        p.files.insert(to.clone(), Some(current_to.replacen(&moved_text, &new_fn, 1)));
        gone.entry(c.file.clone()).or_default().insert(c.name.clone());
        p.moved.push((c.name.clone(), c.file.clone(), to));
    }
    p.collisions = keep;
    // The function now lives in its new file. The old file is merged again without it, so the
    // result does not depend on which side did the moving.
    for (file, names) in &gone {
        let (tb, to, tt) = (rev(&base).read(file), rev(ours).read(file), rev(theirs).read(file));
        if let (Some(to), Some(tt)) = (to, tt) {
            let bad_here: BTreeSet<String> = bad.iter().filter(|(f, _)| f == file).map(|(_, n)| n.clone()).collect();
            p.files.insert(file.clone(), merge_file_with(file, tb.as_deref(), &to, &tt, &bad_here, names)?.text);
        }
    }
    p.collisions.sort_by(|a, b| (&a.file, &a.name).cmp(&(&b.file, &b.name)));
    p.tree = write_tree(v, ours, &p)?;
    if quiet {
        return Ok(p);
    }

    // The merged result, lifted exactly as any commit is.
    let merged_rev = commit_tree(v, &p.tree, &[ours, theirs])?;
    let fresh: BTreeMap<String, String> = v.lift_rev(&merged_rev)?.out.files.clone();

    // Generated files are rebuilt from the merged code, never merged as text.
    if bo.keys().chain(bt.keys()).any(|k| generated(&out, k)) {
        for k in bo.keys().filter(|k| generated(&out, k)) {
            p.files.insert(k.clone(), None);
            p.whole.remove(k);
        }
        for (k, text) in fresh {
            p.files.insert(format!("{out}/{k}"), Some(text));
        }
        p.tree = write_tree(v, ours, &p)?;
    }

    // Where the two sides meet: journeys of the merged result that run changes from both.
    let names = v.names.clone();
    let (lb, lo, lt) = {
        let b = if base.is_empty() { None } else { Some(sid_set(v.lift_rev(&base)?)) };
        let o = sid_set(v.lift_rev(ours)?);
        let t = sid_set(v.lift_rev(theirs)?);
        (b.unwrap_or_default(), o, t)
    };
    let changed = |side: &BTreeMap<String, String>| -> BTreeSet<String> { side.iter().filter(|(k, h)| lb.get(*k) != Some(*h)).map(|(k, _)| k.clone()).collect() };
    let (co, ct) = (changed(&lo), changed(&lt));
    let merged = v.lift_rev(&merged_rev)?;
    for f in &merged.flows {
        let sids: BTreeSet<String> = f.steps.iter().map(|s| merged.repo.sid(s.at)).collect();
        // Each side must bring a step the other did not touch; a step both changed is a collision instead.
        let (so, st): (BTreeSet<String>, BTreeSet<String>) = (sids.intersection(&co).cloned().collect(), sids.intersection(&ct).cloned().collect());
        let (mo, mt): (Vec<String>, Vec<String>) = (so.difference(&st).cloned().collect(), st.difference(&so).cloned().collect());
        if !mo.is_empty() && !mt.is_empty() {
            p.meet_ids.push(f.id.clone());
            p.meets.push((journey_title(&names, &f.id), mo.iter().map(|s| label_of(&names, s)).collect(), mt.iter().map(|s| label_of(&names, s)).collect()));
        }
    }
    let ours_lift = v.lift_rev(ours)?;
    for c in p.collisions.iter_mut() {
        let sid = format!("{}#{}", c.file, c.name);
        c.label = if c.name.is_empty() { c.file.clone() } else { label_of(&names, &sid) };
        c.journeys = ours_lift.flows.iter().filter(|f| f.steps.iter().any(|s| ours_lift.repo.sid(s.at) == sid)).map(|f| journey_title(&names, &f.id)).collect();
    }
    Ok(p)
}

/// Write the merge into the working folder. Collisions keep our version until resolved.
pub fn apply(v: &Vcs, p: &Plan, theirs_name: &str) -> Result<()> {
    apply_checked(v, p, theirs_name, vec![])
}

pub fn apply_checked(v: &Vcs, p: &Plan, theirs_name: &str, checks: Vec<crate::scenario::Check>) -> Result<()> {
    if let Some(h) = &p.blocked {
        bail!("{} cannot merge all at once: {} collides. Merge the branches one at a time, deciding collisions as you go", theirs_name, &h[..8]);
    }
    if v.merge_state_path().exists() || v.git_path("MERGE_HEAD").exists() {
        bail!("a merge is already in progress: finish it with `flow save`, or undo it with `flow merge --abort`");
    }
    let dirty = git(&v.root, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        bail!("save or put away your unsaved changes before merging");
    }
    if p.fast_forward {
        git(&v.root, &["merge", "-q", "--ff-only", &p.theirs])?;
        return Ok(());
    }
    // Git does the writing, so modes and links come out right, and it refuses to overwrite a file
    // it does not track.
    let _ = git(&v.root, &["update-index", "-q", "--refresh"]);
    git(&v.root, &["read-tree", "-m", "-u", "HEAD", &p.tree]).context("the merge would overwrite files that are not saved in git. Move them away and try again")?;
    let (before, after) = (blobs(&v.root, &p.ours)?, blobs(&v.root, &p.tree)?);
    let written: Vec<String> = before.keys().chain(after.keys()).filter(|k| before.get(*k) != after.get(*k)).cloned().collect::<BTreeSet<_>>().into_iter().collect();
    let heads: String = p.heads.iter().map(|h| format!("{h}\n")).collect();
    std::fs::write(v.git_path("MERGE_HEAD"), heads)?;
    std::fs::write(v.git_path("MERGE_MSG"), format!("Merge {theirs_name}\n"))?;
    let st = MergeState { theirs: p.theirs.clone(), theirs_name: theirs_name.to_string(), written, collisions: p.collisions.clone(), checks, accepted: vec![] };
    std::fs::write(v.merge_state_path(), serde_json::to_string_pretty(&st)?)?;
    Ok(())
}

/// Put these paths back as they are in `rev`: staged and on disk, removed if `rev` lacks them.
fn restore(v: &Vcs, rev: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let list = paths.join("\0") + "\0";
    let source = format!("--source={rev}");
    run(&v.root, &["--literal-pathspecs", "restore", &source, "--staged", "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"], &[], Some(&list))?;
    Ok(())
}

/// Replace or remove one function, found by name. Two functions can have identical text, so
/// searching for the text could change the wrong one.
fn swap_unit(path: &str, text: &str, name: &str, with: Option<&str>) -> Option<String> {
    let us = units(&parse_file(path, text, &[]).ok()?);
    if !us.iter().any(|u| matches!(u, Unit::Code(n, ..) if n == name)) {
        return None;
    }
    let parts = us.iter().filter_map(|u| match u {
        Unit::Code(n, ..) if n == name => with.map(str::to_string),
        Unit::Code(_, t, _) | Unit::Line(t) => Some(t.clone()),
    });
    Some(parts.collect::<Vec<_>>().join("\n"))
}

/// Decide one collision: keep ours, or take theirs.
pub fn resolve(v: &Vcs, what: &str, side: &str) -> Result<String> {
    let path = v.merge_state_path();
    if !path.exists() {
        bail!("no merge in progress");
    }
    let mut st: MergeState = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let w = what.to_lowercase();
    let i = st
        .collisions
        .iter()
        .position(|c| !c.resolved && (c.label.to_lowercase() == w || c.name.to_lowercase() == w || c.file.to_lowercase() == w))
        .or_else(|| st.collisions.iter().position(|c| !c.resolved && c.label.to_lowercase().contains(&w)));
    let Some(i) = i else { bail!("no open collision matches {what:?}") };
    let c = st.collisions[i].clone();
    if side == "theirs" {
        if c.name.is_empty() {
            // Whole files come from git, so anything that is not text survives byte for byte, and
            // only a file truly absent on their side is removed.
            restore(v, &st.theirs, std::slice::from_ref(&c.file))?;
            if !st.written.contains(&c.file) {
                st.written.push(c.file.clone());
            }
        } else {
            let full = v.root.join(&c.file);
            let cur = std::fs::read_to_string(&full)?;
            let next = match (&c.ours, &c.theirs) {
                (Some(o), Some(t)) => swap_unit(&c.file, &cur, &c.name, Some(t)).unwrap_or_else(|| cur.replacen(o.as_str(), t, 1)),
                (Some(o), None) => swap_unit(&c.file, &cur, &c.name, None).unwrap_or_else(|| cur.replacen(o.as_str(), "", 1)),
                (None, Some(t)) => format!("{}\n{t}", cur.trim_end_matches('\n')) + "\n",
                (None, None) => cur,
            };
            std::fs::write(&full, next)?;
        }
    } else if side != "ours" {
        bail!("choose ours or theirs");
    }
    st.collisions[i].resolved = true;
    std::fs::write(&path, serde_json::to_string_pretty(&st)?)?;
    Ok(c.label)
}

/// A person accepts what the merge does in a scenario. It is recorded and checked from then on.
pub fn accept(v: &Vcs, name: &str) -> Result<()> {
    let path = v.merge_state_path();
    if !path.exists() {
        bail!("no merge in progress");
    }
    let mut st: MergeState = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let Some(c) = st.checks.iter().find(|c| c.scenario == name) else { bail!("no scenario called {name} ran in this merge") };
    let mut o = c.merged.clone();
    for k in &c.unstable {
        o.remove(k);
    }
    crate::scenario::record(&v.root, &v.out, name, &o)?;
    st.accepted.push(name.to_string());
    std::fs::write(&path, serde_json::to_string_pretty(&st)?)?;
    Ok(())
}

pub fn abort(v: &Vcs) -> Result<()> {
    let path = v.merge_state_path();
    if !path.exists() {
        bail!("no merge in progress");
    }
    let st: MergeState = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    restore(v, "HEAD", &st.written)?;
    for f in ["MERGE_HEAD", "MERGE_MSG"] {
        let _ = std::fs::remove_file(v.git_path(f));
    }
    std::fs::remove_file(path)?;
    Ok(())
}
