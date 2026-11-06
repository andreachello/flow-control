//! `flow serve`: the projects on this machine in a browser, on localhost. Each project opens to
//! its changes, and each change to its files and the journeys it reaches. Everything comes from
//! git and from lifts in memory. The one thing that writes to a repo is a run someone starts from
//! the page, and it takes a request only this page can send, for a task on a fixed list.

use crate::change::{journey_title, FnChange, JState, Report};
use crate::ids::Ids;
use crate::journeys::{self, Journeys, Places};
use crate::titles::{self, Titles};
use crate::names::Names;
use crate::reports::{self, Store};
use crate::repo::SRef;
use crate::runs::{self, Runs};
use crate::soba;
use crate::auth::{self, Auth};
use crate::source::{all_paths, git, Source};
use crate::vcs::{Commit, Vcs};
use crate::{lift_source, Lift};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const PAGE: &str = include_str!("serve.html");
/// What a commit with no parent is compared with: git's id for an empty folder.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// A file longer than this is cut short in a diff, so one lockfile cannot stop the page.
const MOST_ROWS: usize = 3000;
const PAGE_OF_COMMITS: usize = 40;
const LIFTS_KEPT: usize = 24;
const PULLS_KEPT_FOR: Duration = Duration::from_secs(60);
/// Past this a pull request is read from its newest commits, so one huge branch cannot stop the page.
const MOST_PULL_COMMITS: usize = 60;

struct Project {
    name: String,
    root: PathBuf,
    v: Vcs,
    /// A commit never changes, so what was worked out about one stands.
    details: HashMap<String, Value>,
    summaries: HashMap<String, Value>,
    /// What each change did, kept across restarts and journey runs.
    store: Store,
    book: Journeys,
    places: Places,
    ids: Ids,
    /// When the journeys were last read, to notice `flow journeys` finishing while this runs.
    read: Option<[Option<SystemTime>; 3]>,
    /// Every request at the newest commit, worked out once per commit.
    catalog: Option<(String, Value)>,
    titles: Titles,
    /// The list from GitHub, kept briefly: asking takes a second or more.
    pulled: Option<(Instant, Value)>,
    /// A newer list is on its way, so nobody starts another.
    pulling: bool,
}

fn word(s: &JState) -> &'static str {
    match s {
        JState::New => "new",
        JState::Gone => "gone",
        JState::Changed => "changed",
    }
}

fn commit_json(c: &Commit) -> Value {
    json!({ "id": c.id, "date": c.date, "author": c.author, "title": c.subject, "merge": c.parents.len() > 1 })
}

impl Project {
    fn open(root: &Path, out: &str, taken: &mut HashMap<String, usize>) -> Result<Project> {
        let root = std::fs::canonicalize(root).with_context(|| format!("no folder at {}", root.display()))?;
        let names = Names::load(&root.join("flow.names.json"))?;
        let v = Vcs::open(&root, out, names)?;
        let base = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
        let n = taken.entry(base.clone()).or_insert(0);
        *n += 1;
        let name = if *n == 1 { base } else { format!("{base}-{n}") };
        let store = Store::open(&root);
        let mut p = Project { name, root, v, store, details: HashMap::new(), summaries: HashMap::new(), book: Journeys::default(), places: Places::default(), ids: Ids::default(), read: None, catalog: None, titles: Titles::new(), pulled: None, pulling: false };
        p.refresh()?;
        Ok(p)
    }

    fn total(&self) -> usize {
        git(&self.root, &["rev-list", "--count", "HEAD"]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    }

    /// Journeys are edited by hand and by `flow journeys`, so a running server keeps up.
    fn refresh(&mut self) -> Result<()> {
        let at = |f: &str| std::fs::metadata(self.root.join(f)).and_then(|m| m.modified()).ok();
        let now = [at(journeys::FILE), at(crate::ids::FILE), at(titles::FILE)];
        if self.read != Some(now) {
            self.book = Journeys::load(&self.root.join(journeys::FILE))?;
            self.titles = titles::load(&self.root.join(titles::FILE))?;
            let (ids, _) = Ids::parse(&std::fs::read_to_string(self.root.join(crate::ids::FILE)).unwrap_or_default());
            self.places = self.book.places(&ids);
            self.ids = ids;
            self.read = Some(now);
            self.details.clear();
            self.summaries.clear();
            // Which journey a request is in lives in those files, not in the commit.
            self.catalog = None;
        }
        Ok(())
    }

    /// A change as the page shows it: its title where one was written, else its message.
    fn shown(&self, c: &Commit) -> Value {
        let mut j = commit_json(c);
        j["subject"] = json!(c.subject);
        if let Some(t) = self.titles.get(&c.id) {
            j["title"] = json!(t.title);
            j["quiet"] = json!(t.title == titles::QUIET);
        }
        j
    }

    /// The journeys one changed function reaches.
    fn reach(&self, r: &Report, f: &FnChange) -> BTreeSet<usize> {
        let of = |t: &String| r.journeys.iter().find(|j| j.title == *t).and_then(|j| self.places.of(&j.uid, &j.id));
        f.journeys.iter().filter_map(of).collect()
    }

    fn focus(&self, r: &Report) -> Vec<usize> {
        let code = r.functions.iter().filter(|f| f.state != "reformatted only" && !crate::output::is_test_file(&f.sid));
        journeys::focus(&code.map(|f| self.reach(r, f)).collect::<Vec<_>>()).into_iter().collect()
    }

    /// The written clauses of a change, each with the journeys its step reaches.
    fn parts(&self, id: &str, r: &Report) -> Vec<Value> {
        let Some(t) = self.titles.get(id) else { return vec![] };
        let hit = |p: &titles::Part| r.functions.iter().find(|f| f.sid == p.step).map(|f| self.reach(r, f)).unwrap_or_default();
        // Which requests run it, to place the clause at a stop of the journey.
        let runs = |p: &titles::Part| -> Vec<String> {
            let f = r.functions.iter().find(|f| f.sid == p.step);
            f.into_iter().flat_map(|f| f.journeys.iter()).filter_map(|t| r.journeys.iter().find(|j| j.title == *t)).map(|j| j.uid.clone()).collect()
        };
        t.parts.iter().map(|p| json!({ "say": p.say, "line": p.line, "at": p.at, "before": p.before, "after": p.after, "step": p.step, "groups": hit(p), "requests": runs(p) })).collect()
    }

    /// A branch nobody fetched is not on this machine, and this never fetches it: the page says so instead.
    fn pull(&self, pull: Value, ids: Vec<String>) -> Result<Value> {
        let total = ids.len();
        let here = |id: &&String| git(&self.root, &["cat-file", "-e", &format!("{id}^{{commit}}")]).is_ok();
        let found: Vec<&String> = ids.iter().filter(here).collect();
        let from = found.len().saturating_sub(MOST_PULL_COMMITS);
        let commits = found[from..].iter().map(|id| self.commit(id).map(|c| self.shown(&c))).collect::<Result<Vec<_>>>()?;
        Ok(json!({ "pull": pull, "commits": commits, "total": total, "missing": total - found.len(), "cut": from }))
    }

    /// Who changed what and when, for every change. This comes straight from git, so it is quick.
    fn activity(&self) -> Result<Value> {
        let text = if self.v.head().is_some() { git(&self.root, &["log", "-n5000", "--date=short", "--format=%ad%x1f%an", "HEAD"])? } else { String::new() };
        Ok(Value::Array(text.lines().filter_map(|l| l.split_once('\x1f')).map(|(d, a)| json!([d, a])).collect()))
    }

    /// Every request the code has today, and which journey it belongs to.
    fn requests(&mut self) -> Result<Value> {
        self.refresh()?;
        let Some(head) = self.v.head() else { return Ok(json!([])) };
        if let Some((at, v)) = &self.catalog {
            if *at == head {
                return Ok(v.clone());
            }
        }
        let names = self.v.names.clone();
        let l = self.v.lift_rev(&head)?;
        let list = l.flows.iter().filter(|f| !self.book.ignored(&l.repo.files[f.entry.0].path)).map(|f| json!({ "uid": f.uid, "id": f.id, "title": journey_title(&names, &f.id), "kind": f.kind, "steps": f.steps.len(), "group": self.places.of(&f.uid, &f.id), "product": self.book.product_of(&l.repo.files[f.entry.0].path) })).collect();
        let v = Value::Array(list);
        self.catalog = Some((head, v.clone()));
        self.trim();
        Ok(v)
    }

    /// One request as it runs today, step by step.
    fn request(&mut self, uid: &str) -> Result<Value> {
        self.refresh()?;
        let head = self.v.head().context("nothing saved yet")?;
        let names = self.v.names.clone();
        let l = self.v.lift_rev(&head)?;
        let f = l.flows.iter().find(|f| f.uid == uid).context("no such request")?;
        let short = |k: &String| k.rsplit('#').next().unwrap_or(k).to_string();
        let steps: Vec<Value> = f.steps.iter().map(|s| {
            let sid = l.repo.sid(s.at);
            let name = names.steps.get(&sid).map(|n| n.name.clone()).unwrap_or_else(|| l.repo.slice(s.at).name.clone());
            json!({ "name": name, "file": sid.split('#').next().unwrap_or(""), "depth": s.depth })
        }).collect();
        let of = |m: &HashMap<SRef, BTreeSet<String>>| f.steps.iter().filter_map(|s| m.get(&s.at)).flatten().map(short).collect::<BTreeSet<_>>();
        Ok(json!({ "uid": f.uid, "id": f.id, "title": journey_title(&names, &f.id), "kind": f.kind, "group": self.places.of(&f.uid, &f.id), "steps": steps,
            "reads": of(&l.graph.reads), "writes": of(&l.graph.writes), "outside": f.outside }))
    }

    fn legend(&self) -> Vec<Value> {
        self.book.journeys.iter().map(|j| {
            let stops: Vec<Value> = j.stops.iter().map(|s| json!({ "say": s.say, "requests": s.requests.iter().map(|r| self.ids.journey(r)).collect::<Vec<_>>() })).collect();
            json!({ "title": j.title, "icon": j.icon, "stops": stops, "product": j.product })
        }).collect()
    }

    /// The products that have journeys. A folder nobody has walked yet is nothing to pick.
    fn products(&self) -> Vec<&str> {
        self.book.products.iter().map(|p| p.name.as_str()).filter(|p| self.book.journeys.iter().any(|j| j.product.as_deref() == Some(p))).collect()
    }

    fn card(&self) -> Value {
        let recent = self.v.head().and_then(|h| self.v.commits(20, &h).ok()).unwrap_or_default();
        let shown: Vec<Value> = recent.iter().map(|c| self.shown(c)).collect();
        // The latest change anyone would notice is what a project is up to.
        let says = shown.iter().find(|c| c["quiet"] != json!(true)).or(shown.first()).map(|c| c["title"].clone());
        json!({ "name": self.name, "path": self.root.display().to_string(), "changes": self.total(), "last": shown.first(), "says": says, "journeys": self.legend(), "products": self.products(), "recent": shown.iter().take(6).collect::<Vec<_>>() })
    }

    fn sha(&self, s: &str) -> Result<String> {
        if s.len() < 7 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("not a commit id");
        }
        // A whole id already names its commit, and asking git for each adds up across a page.
        if s.len() == 40 {
            return Ok(s.to_string());
        }
        self.v.resolve(s)
    }

    fn commit(&self, sha: &str) -> Result<Commit> {
        self.v.commits(1, sha)?.into_iter().next().ok_or_else(|| anyhow!("no such commit"))
    }

    fn commits(&mut self, skip: usize) -> Result<Value> {
        self.refresh()?;
        let list = if self.v.head().is_some() { self.v.commits_from(skip, PAGE_OF_COMMITS, "HEAD")? } else { vec![] };
        Ok(json!({ "project": self.card(), "skip": skip, "size": PAGE_OF_COMMITS, "commits": list.iter().map(|c| self.shown(c)).collect::<Vec<_>>(),
            "journeys": self.legend(), "map": self.book.map, "maps": self.book.maps, "products": self.products(), "stamp": self.stamp() }))
    }

    /// Changes when the files behind what a page shows do, so a browser knows what it kept is out of date.
    fn stamp(&self) -> u64 {
        let at = |f: &str| std::fs::metadata(self.root.join(f)).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        at(journeys::FILE) ^ at(titles::FILE).rotate_left(21)
    }

    fn trim(&mut self) {
        if self.v.cached_count() > LIFTS_KEPT {
            self.v.forget();
        }
    }

    /// What a change reaches, for the list. Lifted together across the machine's cores.
    fn summaries(&mut self, ids: &[String]) -> Result<Value> {
        let mut shas = vec![];
        let mut unread = vec![];
        for id in ids {
            let sha = self.sha(id)?;
            if !self.summaries.contains_key(&sha) && !self.store.has(&sha) {
                unread.push(self.commit(&sha)?);
            }
            shas.push(sha);
        }
        reports::read_batch(&mut self.v, &self.store, &unread)?;
        let mut out = Map::new();
        for (id, sha) in ids.iter().zip(&shas) {
            out.insert(id.clone(), self.summary(sha)?);
        }
        self.trim();
        Ok(Value::Object(out))
    }

    /// Every change here that is already worked out, all at once, and never a wait for the rest.
    /// A page asks this first, so what is known arrives together instead of a few at a time.
    fn saved(&mut self, ids: &[String]) -> Result<Value> {
        let mut out = Map::new();
        for id in ids {
            let sha = self.sha(id)?;
            if self.summaries.contains_key(&sha) || self.store.has(&sha) {
                out.insert(id.clone(), self.summary(&sha)?);
            }
        }
        Ok(Value::Object(out))
    }

    /// One change as the list shows it: what was kept, with today's journeys applied.
    fn summary(&mut self, sha: &str) -> Result<Value> {
        if let Some(s) = self.summaries.get(sha) {
            return Ok(s.clone());
        }
        let r = match self.store.get(sha) {
            Some(r) => r,
            // With nowhere to keep it, or a file that went bad.
            None => {
                let c = self.commit(sha)?;
                self.v.change_of(&c)?
            }
        };
        let names = &self.v.names;
        let journeys: Vec<Value> = r.journeys.iter().map(|j| json!({ "uid": j.uid, "title": journey_title(names, &j.id), "state": word(&j.state), "was": j.was, "group": self.places.of(&j.uid, &j.id) })).collect();
        let s = json!({ "journeys": journeys, "untouched": r.untouched, "line": crate::change::one_line(&r), "focus": self.focus(&r), "parts": self.parts(sha, &r) });
        self.summaries.insert(sha.to_string(), s.clone());
        Ok(s)
    }

    fn generated(&self, p: &str) -> bool {
        p.starts_with(&format!("{}/", self.v.out)) || p == crate::ids::FILE || p == "flow.names.json" || p == crate::words::FILE || p == journeys::FILE || p == titles::FILE
    }

    fn base_of(c: &Commit) -> String {
        c.parents.first().cloned().unwrap_or_else(|| EMPTY_TREE.to_string())
    }

    fn detail(&mut self, id: &str) -> Result<Value> {
        let sha = self.sha(id)?;
        if let Some(d) = self.details.get(&sha) {
            return Ok(d.clone());
        }
        let c = self.commit(&sha)?;
        let r = self.v.change_of(&c)?;
        let names = self.v.names.clone();
        let empty = lift_source(&Source::Mem { root: self.root.clone(), files: Default::default() }, &self.v.out)?;
        let after = self.v.cached(&c.id).context("lifted above")?;
        let before = c.parents.first().and_then(|p| self.v.cached(p)).unwrap_or(&empty);

        let text = git(&self.root, &diff_args(&Self::base_of(&c), &c.id, 3, &[]).iter().map(String::as_str).collect::<Vec<_>>())?;
        let (old, new) = (owners(before), owners(after));
        let (mut files, mut hidden) = (vec![], 0);
        for f in parse_diff(&text, &old, &new) {
            if self.generated(&f.path) {
                hidden += 1;
            } else {
                files.push(f.json());
            }
        }

        // The saved sentence for a journey, where one was written and reviewed.
        let said = crate::comment::write(before, after, &r, None, None);
        let mut says: BTreeMap<String, String> = BTreeMap::new();
        for i in said.not_asked.iter().chain(&said.changed) {
            if let Some(s) = &i.sentence {
                says.insert(i.title.clone(), s.clone());
                says.extend(i.also.iter().map(|t| (t.clone(), s.clone())));
            }
        }
        let changes: Vec<Value> = r.functions.iter().filter(|f| f.state != "reformatted only").map(|f| {
            let at = crate::words::slice_of(after, &f.sid).map(|x| (after, x)).or_else(|| crate::words::slice_of(before, &f.sid).map(|x| (before, x)));
            let note = f.facts.first().cloned().unwrap_or_else(|| f.state.clone());
            json!({ "sid": f.sid, "label": f.label, "file": f.sid.split('#').next().unwrap_or(""), "state": f.state, "note": note, "facts": f.facts, "journeys": f.journeys, "kept": at.is_some(), "test": crate::output::is_test_file(&f.sid) })
        }).collect();
        let journeys: Vec<Value> = r.journeys.iter().map(|j| {
            let mut steps = crate::map::journey_steps(before, after, &j.id, &j.title, &r, &names);
            for s in steps.iter_mut() {
                if let Value::Object(o) = s {
                    o.remove("before");
                    o.remove("after");
                }
            }
            json!({ "uid": j.uid, "id": j.id, "title": j.title, "state": word(&j.state), "was": j.was, "says": says.get(&j.title), "steps": steps, "group": self.places.of(&j.uid, &j.id),
                "area": names.journeys.get(&j.id).map(|n| n.area.clone()) })
        }).collect();
        let touched: Vec<&str> = r.journeys.iter().map(|j| j.uid.as_str()).collect();
        let mut others: Vec<String> = after.flows.iter().filter(|f| !touched.contains(&f.uid.as_str())).map(|f| journey_title(&names, &f.id)).collect();
        others.sort();
        let tree: Vec<String> = all_paths(&self.root, &c.id)?.into_iter().filter(|p| !self.generated(p)).collect();
        let head = r.journeys.iter().find_map(|j| says.get(&j.title).cloned());
        let body = git(&self.root, &["log", "-1", "--format=%b", &c.id]).unwrap_or_default();
        let body: String = body.lines().take_while(|l| !l.starts_with("Flow-Journeys:")).collect::<Vec<_>>().join("\n");
        let d = json!({ "commit": self.shown(&c), "head": head, "body": body.trim(), "files": files, "hidden": hidden, "changes": changes, "journeys": journeys, "others": others, "tree": tree,
            "labels": labels(before, after, &names), "smaller": said.smaller, "parts": self.parts(&c.id, &r) });
        self.details.insert(sha, d.clone());
        self.trim();
        Ok(d)
    }

    /// Every line of a file as it stood at a commit, with the owner of each.
    fn file(&mut self, id: &str, path: &str) -> Result<Value> {
        let sha = self.sha(id)?;
        if !all_paths(&self.root, &sha)?.iter().any(|p| p == path) {
            bail!("no such file in this commit");
        }
        let text = git(&self.root, &["show", &format!("{sha}:{path}")])?;
        let l = self.v.lift_rev(&sha)?;
        let own = owners(l).remove(path).unwrap_or_default();
        let rows: Vec<Value> = text.lines().enumerate().map(|(i, t)| json!([" ", i + 1, i + 1, t, null, own.get(i).cloned().flatten()])).collect();
        Ok(json!({ "rows": rows.into_iter().take(MOST_ROWS * 4).collect::<Vec<_>>() }))
    }

    /// One changed file with every line around the change.
    fn whole(&mut self, id: &str, path: &str, from: Option<&str>) -> Result<Value> {
        let sha = self.sha(id)?;
        let c = self.commit(&sha)?;
        let base = Self::base_of(&c);
        self.v.lift_rev(&sha)?;
        if let Some(p) = c.parents.first() {
            self.v.lift_rev(p)?;
        }
        let empty = lift_source(&Source::Mem { root: self.root.clone(), files: Default::default() }, &self.v.out)?;
        let (after, before) = (self.v.cached(&sha).context("lifted above")?, c.parents.first().and_then(|p| self.v.cached(p)).unwrap_or(&empty));
        let mut paths = vec![path];
        paths.extend(from);
        let args = diff_args(&base, &sha, 1_000_000, &paths);
        let text = git(&self.root, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        let f = parse_diff(&text, &owners(before), &owners(after)).into_iter().find(|f| f.path == path).context("no such change")?;
        Ok(json!({ "rows": f.rows }))
    }
}

fn diff_args(base: &str, head: &str, context: usize, paths: &[&str]) -> Vec<String> {
    // Everything named here so nobody's personal git settings change what is shown.
    let mut a: Vec<String> = ["-c", "core.quotepath=off", "diff", "--no-color", "--no-ext-diff", "--no-textconv", "-M", "--src-prefix=a/", "--dst-prefix=b/"].iter().map(|s| s.to_string()).collect();
    a.push(format!("-U{context}"));
    a.extend([base.to_string(), head.to_string()]);
    if !paths.is_empty() {
        a.push("--".into());
        a.extend(paths.iter().map(|s| s.to_string()));
    }
    a
}

/// path -> the step that owns each line.
fn owners(l: &Lift) -> HashMap<String, Vec<Option<String>>> {
    let mut m = HashMap::new();
    for (fi, f) in l.repo.files.iter().enumerate() {
        let mut v: Vec<Option<String>> = vec![None; f.lines.len()];
        for si in (0..f.slices.len()).filter(|&s| s != crate::parse::SUPPORT) {
            for &n in &f.slices[si].lines {
                if n < v.len() {
                    v[n] = Some(l.repo.sid((fi, si)));
                }
            }
        }
        m.insert(f.path.clone(), v);
    }
    m
}

fn labels(a: &Lift, b: &Lift, names: &Names) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for l in [a, b] {
        for (fi, f) in l.repo.files.iter().enumerate() {
            for si in (0..f.slices.len()).filter(|&s| s != crate::parse::SUPPORT && !f.slices[s].lines.is_empty()) {
                m.insert(l.repo.sid((fi, si)), crate::change::step_label(names, l, (fi, si)));
            }
        }
    }
    m
}

#[derive(Default)]
struct FileDiff {
    path: String,
    from: Option<String>,
    status: &'static str,
    add: usize,
    del: usize,
    binary: bool,
    cut: bool,
    rows: Vec<Value>,
}

impl FileDiff {
    fn json(self) -> Value {
        json!({ "path": self.path, "from": self.from, "status": self.status, "add": self.add, "del": self.del, "binary": self.binary, "cut": self.cut, "rows": self.rows })
    }
}

type Owners = HashMap<String, Vec<Option<String>>>;

/// Rows of `[sign, old line, new line, text, null, owner]`, the shape the page draws.
fn parse_diff(text: &str, old: &Owners, new: &Owners) -> Vec<FileDiff> {
    let owner = |m: &Owners, p: &str, n: usize| m.get(p).and_then(|v| v.get(n.wrapping_sub(1))).cloned().flatten();
    let (mut out, mut cur): (Vec<FileDiff>, Option<FileDiff>) = (vec![], None);
    let (mut a, mut b) = (0usize, 0usize);
    let mut in_hunk = false;
    for line in text.split('\n') {
        if line.starts_with("diff --git ") {
            out.extend(cur.take());
            let path = line.rsplit_once(" b/").map(|x| x.1.to_string()).unwrap_or_default();
            cur = Some(FileDiff { path, status: "M", ..Default::default() });
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if !in_hunk {
            if line.starts_with("new file") {
                f.status = "A";
            } else if line.starts_with("deleted file") {
                f.status = "D";
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.from = Some(p.to_string());
                f.status = "R";
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = p.to_string();
            } else if line.starts_with("Binary files") {
                f.binary = true;
            } else if let Some(p) = line.strip_prefix("+++ b/") {
                f.path = p.to_string();
            } else if let Some(p) = line.strip_prefix("--- a/").filter(|_| f.status == "D") {
                f.path = p.to_string();
            }
        }
        if let Some(h) = line.strip_prefix("@@ ") {
            let mut it = h.split_whitespace();
            let num = |s: Option<&str>| s.and_then(|x| x[1..].split(',').next().and_then(|n| n.parse::<usize>().ok())).unwrap_or(0);
            (a, b) = (num(it.next()), num(it.next()));
            in_hunk = true;
            if f.rows.len() < MOST_ROWS {
                f.rows.push(json!(["@", line]));
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (old_path, new_path) = (f.from.clone().unwrap_or_else(|| f.path.clone()), f.path.clone());
        let (sign, body) = match line.chars().next() {
            Some(c @ ('+' | '-' | ' ')) => (c, &line[1..]),
            _ => continue,
        };
        let row = match sign {
            '+' => {
                f.add += 1;
                b += 1;
                json!(["+", "", b - 1, body, null, owner(new, &new_path, b - 1)])
            }
            '-' => {
                f.del += 1;
                a += 1;
                json!(["-", a - 1, "", body, null, owner(old, &old_path, a - 1)])
            }
            _ => {
                a += 1;
                b += 1;
                json!([" ", a - 1, b - 1, body, null, owner(new, &new_path, b - 1)])
            }
        };
        if f.rows.len() < MOST_ROWS {
            f.rows.push(row);
        } else {
            f.cut = true;
        }
    }
    out.extend(cur);
    out
}

fn unescape(s: &str) -> String {
    let (b, mut out, mut i) = (s.as_bytes(), vec![], 0);
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(x) => {
                    out.push(x);
                    i += 3;
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn query(q: &str) -> HashMap<String, String> {
    q.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), unescape(v))).collect()
}

fn reply(s: &mut TcpStream, status: &str, kind: &str, body: &str) {
    answer(s, status, kind, "", body);
}

fn answer(s: &mut TcpStream, status: &str, kind: &str, more: &str, body: &str) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{more}Connection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes()));
}

fn go_to(s: &mut TcpStream, to: &str, cookies: &[String]) {
    let set: String = cookies.iter().map(|c| format!("Set-Cookie: {c}\r\n")).collect();
    answer(s, "302 Found", "text/plain", &format!("Location: {to}\r\n{set}"), "");
}

fn signing_in(s: &mut TcpStream, a: &Auth) {
    match a.start() {
        Ok((to, state)) => go_to(s, &to, &[format!("{}={state}; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=600", auth::STATE)]),
        Err(e) => reply(s, "400 Bad Request", "text/plain", &e.to_string()),
    }
}

/// Each project beside its name, so finding one never waits on another that is busy.
type Shared = Arc<Vec<(String, Mutex<Project>)>>;

/// GitHub can be away for a moment. What it said last stands until it says otherwise.
fn pull_list(projects: &Shared, at: usize, root: &Path) -> Value {
    let v = crate::pulls::list(root);
    let mut p = projects[at].1.lock().unwrap_or_else(|e| e.into_inner());
    p.pulling = false;
    let away = v["linked"] != json!(true) && p.pulled.as_ref().is_some_and(|(_, was)| was["linked"] == json!(true));
    if away {
        let kept = p.pulled.as_ref().map(|(_, was)| was.clone()).unwrap_or(v);
        p.pulled = Some((Instant::now(), kept.clone()));
        return kept;
    }
    p.pulled = Some((Instant::now(), v.clone()));
    v
}

fn api(projects: &Shared, path: &str, q: &HashMap<String, String>) -> Result<Value> {
    if path == "/api/projects" {
        let mut cards = vec![];
        for p in projects.iter() {
            let mut p = p.1.lock().unwrap_or_else(|e| e.into_inner());
            p.refresh()?;
            cards.push(p.card());
        }
        return Ok(Value::Array(cards));
    }
    let rest = path.strip_prefix("/api/p/").context("no such page")?;
    let mut parts = rest.splitn(3, '/');
    let (name, what, arg) = (unescape(parts.next().unwrap_or("")), parts.next().unwrap_or(""), parts.next());
    let at = projects.iter().position(|(n, _)| *n == name).context("no such project")?;
    let cell = &projects[at].1;
    // GitHub can take seconds. Nothing else about this project waits for it.
    let lock = || cell.lock().unwrap_or_else(|e| e.into_inner());
    match (what, arg) {
        ("pulls", None) => {
            // Whoever asks gets the list at once. A stale one is replaced behind them.
            let (kept, stale, root) = {
                let mut p = lock();
                let stale = !p.pulling && p.pulled.as_ref().is_some_and(|(at, _)| at.elapsed() >= PULLS_KEPT_FOR);
                p.pulling |= stale;
                (p.pulled.as_ref().map(|(_, v)| v.clone()), stale, p.root.clone())
            };
            if stale {
                let (all, root) = (projects.clone(), root.clone());
                std::thread::spawn(move || pull_list(&all, at, &root));
            }
            return Ok(match kept {
                Some(v) => v,
                None => pull_list(projects, at, &root),
            });
        }
        ("pull", Some(n)) => {
            let number: u64 = n.parse().context("no such pull request")?;
            let root = lock().root.clone();
            let (pull, ids) = crate::pulls::one(&root, number)?;
            return lock().pull(pull, ids);
        }
        _ => {}
    }
    let mut p = lock();
    let need = |k: &str| q.get(k).cloned().with_context(|| format!("missing {k}"));
    match (what, arg) {
        ("commits", None) => p.commits(q.get("skip").and_then(|s| s.parse().ok()).unwrap_or(0)),
        ("summaries", None) => {
            let ids: Vec<String> = need("ids")?.split(',').map(str::to_string).collect();
            if q.contains_key("saved") { p.saved(&ids) } else { p.summaries(&ids) }
        }
        ("activity", None) => p.activity(),
        ("requests", None) => p.requests(),
        ("request", Some(id)) => p.request(id),
        ("commit", Some(id)) => p.detail(id),
        ("file", Some(id)) => p.file(id, &need("path")?),
        ("whole", Some(id)) => p.whole(id, &need("path")?, q.get("from").map(String::as_str)),
        _ => bail!("no such page"),
    }
}

/// `/api/p/<name>/run` and `/api/p/<name>/run/<task>`. These never wait on a project's lock, so a
/// page keeps hearing how a run is going while something slow reads that project.
fn run_path(path: &str) -> Option<(String, Option<&str>)> {
    let (name, rest) = path.strip_prefix("/api/p/")?.split_once('/')?;
    match rest.strip_prefix("run")? {
        "" => Some((unescape(name), None)),
        task => Some((unescape(name), Some(task.strip_prefix('/')?))),
    }
}

fn said(s: &mut TcpStream, got: Result<Value>) {
    match got {
        Ok(v) => reply(s, "200 OK", "application/json; charset=utf-8", &v.to_string()),
        Err(e) => reply(s, "400 Bad Request", "application/json; charset=utf-8", &json!({ "error": e.to_string() }).to_string()),
    }
}

/// Whether a request came from this app's own page, wherever that page is served from.
fn from_page(origin: &str, hosts: &[String]) -> bool {
    origin.strip_prefix("http://").is_some_and(|o| hosts.iter().any(|h| h == o))
}

fn handle(mut s: TcpStream, projects: Shared, runs: Arc<Runs>, hosts: Arc<Vec<String>>, a: Arc<Auth>) {
    let Ok(clone) = s.try_clone() else { return };
    let mut r = BufReader::new(clone);
    let mut first = String::new();
    if r.read_line(&mut first).is_err() {
        return;
    }
    let (mut host, mut cookies, mut origin) = (String::new(), String::new(), String::new());
    loop {
        let mut l = String::new();
        match r.read_line(&mut l) {
            Ok(n) if n > 2 => {
                if let Some((k, v)) = l.split_once(':') {
                    if k.eq_ignore_ascii_case("host") {
                        host = v.trim().to_string();
                    } else if k.eq_ignore_ascii_case("cookie") {
                        cookies = v.trim().to_string();
                    } else if k.eq_ignore_ascii_case("origin") {
                        origin = v.trim().to_string();
                    }
                }
            }
            _ => break,
        }
    }
    // Source code is served here. A web page pointing its own name at this address must not read it.
    if !hosts.contains(&host) {
        return reply(&mut s, "403 Forbidden", "text/plain", "This address only answers on localhost.");
    }
    let mut it = first.split_whitespace();
    let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or("/"));
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let session = auth::cookie(&cookies, auth::SESSION);
    if method == "POST" && path == "/auth/logout" {
        // Signing someone out is a change, so another site must not be able to ask for it.
        if !from_page(&origin, &hosts) {
            return reply(&mut s, "403 Forbidden", "text/plain", "Not from this page.");
        }
        a.end(session);
        return answer(&mut s, "200 OK", "application/json", &format!("Set-Cookie: {}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0\r\n", auth::SESSION), "{}");
    }
    if let (true, Some((name, Some(task)))) = (method == "POST", run_path(path)) {
        // This starts work on the machine, on someone's own plan, so it is the page's to ask for.
        if !from_page(&origin, &hosts) {
            return reply(&mut s, "403 Forbidden", "application/json; charset=utf-8", &json!({ "error": "Not from this page" }).to_string());
        }
        if !a.allows(session) {
            return reply(&mut s, "401 Unauthorized", "application/json; charset=utf-8", &json!({ "error": "Sign in first" }).to_string());
        }
        // Whose compute this spends comes from the sign-in, and from nowhere the browser can touch.
        let user = a.login(session).filter(|_| soba::ready());
        return said(&mut s, runs.start(&name, task, user));
    }
    if method == "POST" && path == "/api/soba/session" {
        if !from_page(&origin, &hosts) {
            return reply(&mut s, "403 Forbidden", "application/json; charset=utf-8", &json!({ "error": "Not from this page" }).to_string());
        }
        let Some(login) = a.login(session) else {
            return reply(&mut s, "401 Unauthorized", "application/json; charset=utf-8", &json!({ "error": "Sign in first" }).to_string());
        };
        if !soba::ready() {
            return reply(&mut s, "400 Bad Request", "application/json; charset=utf-8", &json!({ "error": "Soba is not set up here" }).to_string());
        }
        return match soba::session(&login) {
            Ok((status, body)) => reply(&mut s, &format!("{status} Soba"), "application/json; charset=utf-8", &body),
            Err(e) => reply(&mut s, "502 Bad Gateway", "application/json; charset=utf-8", &json!({ "error": e.to_string() }).to_string()),
        };
    }
    if method != "GET" {
        return reply(&mut s, "405 Method Not Allowed", "text/plain", "Read only.");
    }
    if path == "/" {
        return reply(&mut s, "200 OK", "text/html; charset=utf-8", PAGE);
    }
    if path == "/auth/github" {
        // GitHub sends people back to one address, and the cookies must be on that one.
        if host == hosts[1] {
            return go_to(&mut s, &format!("http://{}/auth/github", hosts[0]), &[]);
        }
        return signing_in(&mut s, &a);
    }
    if path == "/auth/github/callback" {
        let done = a.finish(&query(q), auth::cookie(&cookies, auth::STATE));
        let mut set = vec![format!("{}=; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=0", auth::STATE)];
        set.extend(done.cookie);
        return go_to(&mut s, done.to, &set);
    }
    if path == "/api/me" {
        let mut me = a.me(session);
        me["soba"] = json!(soba::ready() && a.login(session).is_some());
        return reply(&mut s, "200 OK", "application/json; charset=utf-8", &me.to_string());
    }
    if !a.allows(session) {
        return reply(&mut s, "401 Unauthorized", "application/json; charset=utf-8", &json!({ "error": "Sign in first" }).to_string());
    }
    if !path.starts_with("/api/") {
        return reply(&mut s, "404 Not Found", "text/plain", "Not here.");
    }
    if let Some((name, None)) = run_path(path) {
        return said(&mut s, runs.show(&name));
    }
    said(&mut s, api(&projects, path, &query(q)));
}

/// Serve until stopped. Only this machine can reach it.
///
/// `public` is the port of the web app in front of this, when there is one. People sign in there,
/// so GitHub sends them back there, and this keeps answering only on this machine.
pub fn run(roots: &[PathBuf], out: &str, port: u16, public: Option<u16>) -> Result<()> {
    let mut taken = HashMap::new();
    let projects: Vec<Mutex<Project>> = roots.iter().map(|r| Project::open(r, out, &mut taken).map(Mutex::new)).collect::<Result<_>>()?;
    let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("port {port} is in use. Try --port with another number"))?;
    let port = listener.local_addr()?.port();
    let a = Arc::new(Auth::from_env(public.unwrap_or(port))?);
    println!("Flow control on http://localhost:{port}  ({} project{}{}). Ctrl-C stops it.", projects.len(), if projects.len() == 1 { "" } else { "s" }, if a.required() { ", sign-in required" } else { "" });
    let front = public.unwrap_or(port);
    let mut hosts = vec![format!("localhost:{front}"), format!("127.0.0.1:{front}")];
    if public.is_some() {
        hosts.extend([format!("localhost:{port}"), format!("127.0.0.1:{port}")]);
    }
    let hosts = Arc::new(hosts);
    let roots = projects.iter().map(|p| { let p = p.lock().unwrap_or_else(|e| e.into_inner()); (p.name.clone(), p.root.clone()) }).collect();
    let runs = Arc::new(Runs::new(roots, Arc::new(runs::Here { out: out.to_string() })));
    for p in &projects {
        let p = p.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(v) = Vcs::open(&p.root, out, p.v.names.clone()) {
            let store = p.store.clone();
            std::thread::spawn(move || reports::read_ahead(v, store));
        }
    }
    let projects: Shared = Arc::new(projects.into_iter().map(|m| { let name = m.lock().unwrap_or_else(|e| e.into_inner()).name.clone(); (name, m) }).collect());
    // The two slowest first answers are worked out before anyone opens a page.
    for at in 0..projects.len() {
        let all = projects.clone();
        std::thread::spawn(move || {
            let root = all[at].1.lock().unwrap_or_else(|e| e.into_inner()).root.clone();
            pull_list(&all, at, &root);
            let _ = all[at].1.lock().unwrap_or_else(|e| e.into_inner()).requests();
        });
    }
    for s in listener.incoming().flatten() {
        let (p, r, h, a) = (projects.clone(), runs.clone(), hosts.clone(), a.clone());
        std::thread::spawn(move || handle(s, p, r, h, a));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_keeps_its_line_numbers_and_owners() {
        let text = "diff --git a/a.ts b/a.ts\nindex 1..2 100644\n--- a/a.ts\n+++ b/a.ts\n@@ -1,2 +1,2 @@ fn\n keep\n-old\n+new\n";
        let own = |s: &str| HashMap::from([("a.ts".to_string(), vec![None, Some(s.to_string())])]);
        let f = &parse_diff(text, &own("a.ts#before"), &own("a.ts#after"))[0];
        assert_eq!((f.path.as_str(), f.status, f.add, f.del), ("a.ts", "M", 1, 1));
        assert_eq!(f.rows[2], json!(["-", 2, "", "old", null, "a.ts#before"]));
        assert_eq!(f.rows[3], json!(["+", "", 2, "new", null, "a.ts#after"]));
    }

    #[test]
    fn a_moved_or_binary_file_reads_as_one() {
        let text = "diff --git a/x.ts b/y.ts\nsimilarity index 100%\nrename from x.ts\nrename to y.ts\ndiff --git a/p.png b/p.png\nnew file mode 100644\nBinary files /dev/null and b/p.png differ\n";
        let v = parse_diff(text, &HashMap::new(), &HashMap::new());
        assert_eq!((v[0].path.as_str(), v[0].from.as_deref(), v[0].status), ("y.ts", Some("x.ts"), "R"));
        assert!(v[1].binary && v[1].status == "A");
    }

    #[test]
    fn a_page_is_only_ours_if_it_is_served_from_our_own_addresses() {
        let hosts = ["localhost:4000".to_string(), "127.0.0.1:4000".to_string(), "localhost:4001".to_string()];
        assert!(from_page("http://localhost:4000", &hosts));
        for not in ["", "null", "http://evil.example", "https://localhost:4000", "http://localhost:4000.evil.example"] {
            assert!(!from_page(not, &hosts), "{not}");
        }
    }

    #[test]
    fn only_the_run_addresses_are_run_addresses() {
        assert_eq!(run_path("/api/p/my%20shop/run"), Some(("my shop".to_string(), None)));
        assert_eq!(run_path("/api/p/shop/run/journeys"), Some(("shop".to_string(), Some("journeys"))));
        for not in ["/api/p/shop/runner", "/api/p/shop/commits", "/api/projects", "/api/p/run"] {
            assert_eq!(run_path(not), None, "{not}");
        }
    }

    #[test]
    fn a_query_is_decoded_without_trusting_it() {
        assert_eq!(unescape("src%2Fapp%2F%5Bid%5D+x"), "src/app/[id] x");
        assert_eq!(unescape("100%"), "100%");
        assert_eq!(unescape("caff%C3%A8%zz"), "caffè%zz");
    }
}
