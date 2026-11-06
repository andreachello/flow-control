//! Plain-language names for journeys and steps, proposed by Claude and kept in a committed file.
//!
//! The lifter stays deterministic: names never change what a flow contains. Each name records
//! the hash of the code it describes, so when that code changes the name is shown as stale
//! instead of silently describing something that no longer exists. Edit the file by hand to
//! correct a name; set "by" to "person" and `flow name` will never overwrite it.

use crate::output::hash_of;
use crate::parse::Role;
use crate::util::fnv64;
use crate::Lift;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Names {
    #[serde(default)]
    pub journeys: BTreeMap<String, JourneyName>,
    #[serde(default)]
    pub steps: BTreeMap<String, StepName>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JourneyName {
    pub title: String,
    pub summary: String,
    pub area: String,
    pub hash: String,
    #[serde(default = "claude")]
    pub by: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StepName {
    pub name: String,
    pub hash: String,
    #[serde(default = "claude")]
    pub by: String,
}

fn claude() -> String {
    "claude".into()
}

impl Names {
    pub fn load(p: &Path) -> Result<Names> {
        if !p.exists() {
            return Ok(Names::default());
        }
        let mut n: Names = serde_json::from_str(&std::fs::read_to_string(p)?).with_context(|| format!("reading {}", p.display()))?;
        n.follow(&p.with_file_name(crate::ids::FILE));
        Ok(n)
    }
    /// A name stays with what it names through a rename.
    fn follow(&mut self, ids: &Path) {
        let (ids, _) = crate::ids::Ids::parse(&std::fs::read_to_string(ids).unwrap_or_default());
        for w in &ids.lines {
            let Some(was) = &w.was else { continue };
            for k in ids.lines.iter().filter(|l| l.id == w.id).filter_map(|l| l.key.as_ref()) {
                if let Some(n) = self.journeys.get(was).cloned() {
                    self.journeys.entry(k.clone()).or_insert(n);
                }
                if let Some(n) = self.steps.get(was).cloned() {
                    self.steps.entry(k.clone()).or_insert(n);
                }
            }
        }
    }
    pub fn save(&self, p: &Path) -> Result<()> {
        std::fs::write(p, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }
}

/// A journey's name goes stale when any step in it changes, or steps are added or removed.
pub fn journey_hash(l: &Lift, fi: usize) -> String {
    let f = &l.flows[fi];
    fnv64(&f.steps.iter().map(|s| format!("{}={}", l.repo.sid(s.at), hash_of(&l.repo, s.at))).collect::<Vec<_>>().join("\n"))
}

pub struct Options {
    pub model: String,
    pub jobs: usize,
    pub dry_run: bool,
    pub all: bool,
}

pub fn doc_of(l: &Lift, r: (usize, usize)) -> String {
    let f = &l.repo.files[r.0];
    let mut out = vec![];
    for &n in &l.repo.slice(r).lines {
        let t = f.lines[n].trim();
        if t.is_empty() && out.is_empty() {
            continue;
        }
        if !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')) {
            break;
        }
        let b = t.trim_start_matches("/**").trim_start_matches("/*").trim_start_matches("//").trim_start_matches('*').trim_end_matches("*/").trim();
        if !b.is_empty() {
            out.push(b.to_string());
        }
    }
    out.join(" ")
}

fn code_of(l: &Lift, r: (usize, usize), max: usize) -> String {
    let f = &l.repo.files[r.0];
    let lines = &l.repo.slice(r).lines;
    let mut v: Vec<&str> = lines.iter().map(|&n| f.lines[n].as_str()).filter(|x| !x.trim().is_empty()).collect();
    if v.len() > max {
        v.truncate(max);
        v.push("  …");
    }
    v.join("\n")
}

pub const STYLE: &str = "Write for a product person who does not read code. Plain words about what the person using the product does or gets, never function names, file names or technical terms like query, slug, payload, handler or render. Sentence case. No em dashes. English, even when the code or routes are in another language.";

pub fn run_claude(model: &str, prompt: &str, schema: &Value) -> Result<Value> {
    if let Some(who) = crate::soba::person() {
        return crate::soba::ask(&who, prompt, schema);
    }
    let mut child = Command::new("claude")
        .args(["-p", "--model", model, "--output-format", "json", "--tools", "", "--no-session-persistence", "--strict-mcp-config", "--setting-sources", "", "--effort", "low", "--json-schema"])
        .arg(schema.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start `claude`. Install Claude Code and log in, then run flow name again")?;
    child.stdin.take().unwrap().write_all(prompt.as_bytes())?;
    let out = child.wait_with_output()?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| anyhow!("claude returned something unexpected: {}", String::from_utf8_lossy(&out.stderr).trim()))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        bail!("claude reported an error: {}", v["result"]);
    }
    let so = v["structured_output"].clone();
    if so.is_null() {
        bail!("claude did not return structured output");
    }
    Ok(so)
}

pub fn parallel<T: Send, R: Send>(jobs: usize, items: Vec<T>, f: impl Fn(usize, T) -> R + Sync) -> Vec<R> {
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    let mut queue: Vec<(usize, T)> = items.into_iter().enumerate().collect();
    queue.reverse();
    let queue = std::sync::Mutex::new(queue);
    let results = std::sync::Mutex::new(&mut out);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let next = queue.lock().unwrap().pop();
                let Some((i, t)) = next else { break };
                let r = f(i, t);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    out.into_iter().map(|r| r.unwrap()).collect()
}

pub struct Report {
    pub journeys_named: usize,
    pub steps_named: usize,
    pub calls: usize,
    pub failures: Vec<String>,
}

pub fn name(l: &Lift, names: &mut Names, o: &Options) -> Result<Report> {
    let repo = &l.repo;
    let mut rep = Report { journeys_named: 0, steps_named: 0, calls: 0, failures: vec![] };

    // Journeys whose name is missing or stale. A person's name is never replaced.
    let todo: Vec<usize> = (0..l.flows.len())
        .filter(|&i| match names.journeys.get(&l.flows[i].id) {
            Some(n) if n.by == "person" => false,
            Some(n) => o.all || n.hash != journey_hash(l, i),
            None => true,
        })
        .collect();

    // Every function a journey runs, named once even when many journeys share it.
    let mut seen = BTreeSet::new();
    let mut step_todo = vec![];
    for f in &l.flows {
        for s in &f.steps {
            let sl = repo.slice(s.at);
            if !sl.role.runs() || sl.role == Role::Data || !seen.insert(s.at) {
                continue;
            }
            let sid = repo.sid(s.at);
            let ok = match names.steps.get(&sid) {
                Some(n) if n.by == "person" => true,
                Some(n) => !o.all && n.hash == hash_of(repo, s.at),
                None => false,
            };
            if !ok {
                step_todo.push(s.at);
            }
        }
    }
    let step_batches: Vec<Vec<(usize, usize)>> = step_todo.chunks(45).map(|c| c.to_vec()).collect();
    let journey_batches: Vec<Vec<usize>> = todo.chunks(40).map(|c| c.to_vec()).collect();
    println!(
        "{} journeys and {} steps need names: {} calls to {} ({} at a time).",
        todo.len(),
        step_todo.len(),
        journey_batches.len() + step_batches.len(),
        o.model,
        o.jobs
    );
    if o.dry_run || (todo.is_empty() && step_todo.is_empty()) {
        return Ok(rep);
    }

    // 1. Journeys first, so step names can refer to journeys people recognise.
    let mut areas: BTreeSet<String> = names.journeys.values().map(|j| j.area.clone()).collect();
    let jschema = json!({
        "type": "object", "additionalProperties": false, "required": ["journeys"],
        "properties": {"journeys": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "title", "summary", "area"],
            "properties": {"id": {"type": "string"}, "title": {"type": "string"}, "summary": {"type": "string"}, "area": {"type": "string"}}
        }}}
    });
    for batch in &journey_batches {
        let mut p = format!(
            "These are the user journeys of one web app, read from its code. Give each a name.\n\n{STYLE}\n\n\
             title: at most 6 words, from the point of view of the person, like \"Owner checks their billing\" or \"Guest asks to book a stay\". Pages are things a person looks at; actions are things a person does; requests from other systems (webhooks, cron jobs) say which system.\n\
             summary: one sentence on what happens for the person.\n\
             area: the part of the product it belongs to, like \"Owner dashboard\" or \"Guest booking\". Use 5 to 9 areas for the whole app.{}\n\n",
            if areas.is_empty() { String::new() } else { format!(" Reuse these existing areas where they fit: {}.", areas.iter().cloned().collect::<Vec<_>>().join(", ")) }
        );
        for &i in batch {
            let f = &l.flows[i];
            p += &format!("## {}\nkind: {}\n", f.id, f.kind);
            let d = doc_of(l, f.entry);
            if !d.is_empty() {
                p += &format!("comment: {d}\n");
            }
            let mut main = vec![];
            for s in f.steps.iter().skip(1).filter(|s| s.depth <= 2).take(30) {
                let sl = repo.slice(s.at);
                if sl.role == Role::Data {
                    continue;
                }
                let d = doc_of(l, s.at);
                main.push(format!("  {}{} ({}){}", "  ".repeat(s.depth.saturating_sub(1)), sl.name, s.via, if d.is_empty() { String::new() } else { format!(": {}", crate::util::short(&d, 120)) }));
            }
            if !main.is_empty() {
                p += &format!("steps:\n{}\n", main.join("\n"));
            }
            let w: BTreeSet<String> = f.steps.iter().flat_map(|s| l.graph.writes.get(&s.at).cloned().unwrap_or_default()).map(|k| k.rsplit('#').next().unwrap_or(&k).to_string()).collect();
            if !w.is_empty() {
                p += &format!("changes: {}\n", w.into_iter().collect::<Vec<_>>().join(", "));
            }
            p += "\n";
        }
        rep.calls += 1;
        match run_claude(&o.model, &p, &jschema) {
            Ok(v) => {
                for j in v["journeys"].as_array().cloned().unwrap_or_default() {
                    let id = j["id"].as_str().unwrap_or("");
                    let Some(&i) = batch.iter().find(|&&i| l.flows[i].id == id) else { continue };
                    let area = j["area"].as_str().unwrap_or("Other").to_string();
                    areas.insert(area.clone());
                    names.journeys.insert(
                        id.to_string(),
                        JourneyName {
                            title: j["title"].as_str().unwrap_or(id).to_string(),
                            summary: j["summary"].as_str().unwrap_or("").to_string(),
                            area,
                            hash: journey_hash(l, i),
                            by: "claude".into(),
                        },
                    );
                    rep.journeys_named += 1;
                }
            }
            Err(e) => rep.failures.push(format!("journeys: {e:#}")),
        }
    }

    // 2. Steps, in parallel batches.
    let sschema = json!({
        "type": "object", "additionalProperties": false, "required": ["steps"],
        "properties": {"steps": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "name"],
            "properties": {"id": {"type": "string"}, "name": {"type": "string"}}
        }}}
    });
    let journey_titles = |r: (usize, usize)| -> Vec<String> {
        l.flows
            .iter()
            .filter(|f| f.steps.iter().any(|s| s.at == r))
            .take(3)
            .map(|f| names.journeys.get(&f.id).map(|n| n.title.clone()).unwrap_or_else(|| f.id.clone()))
            .collect()
    };
    let prompts: Vec<(Vec<(usize, usize)>, String)> = step_batches
        .into_iter()
        .map(|batch| {
            let mut p = format!(
                "Each block below is one step that runs inside the user journeys of a web app. Give each step a short plain name.\n\n{STYLE}\n\n\
                 name: at most 7 words, starting with a verb, saying what the step does for the journey, like \"Check the owner is logged in\", \"Work out the price of the stay\" or \"Email the owner about the request\". A visual piece of a page is named for what the person sees, like \"Monthly fee card\". Return every id exactly as given.\n\n"
            );
            for &r in &batch {
                let d = doc_of(l, r);
                p += &format!("## {}\nused in: {}\n", repo.sid(r), journey_titles(r).join("; "));
                if !d.is_empty() {
                    p += &format!("comment: {}\n", crate::util::short(&d, 200));
                }
                p += &format!("```\n{}\n```\n\n", code_of(l, r, 22));
            }
            (batch, p)
        })
        .collect();
    rep.calls += prompts.len();
    let total = prompts.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results = parallel(o.jobs, prompts, |_, (batch, p)| {
        let r = run_claude(&o.model, &p, &sschema);
        let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        eprintln!("  steps: {n} of {total} batches back");
        (batch, r)
    });
    for (batch, r) in results {
        match r {
            Ok(v) => {
                for s in v["steps"].as_array().cloned().unwrap_or_default() {
                    let id = s["id"].as_str().unwrap_or("");
                    let Some(&at) = batch.iter().find(|&&r| repo.sid(r) == id) else { continue };
                    names.steps.insert(id.to_string(), StepName { name: s["name"].as_str().unwrap_or("").to_string(), hash: hash_of(repo, at), by: "claude".into() });
                    rep.steps_named += 1;
                }
            }
            Err(e) => rep.failures.push(format!("steps: {e:#}")),
        }
    }
    Ok(rep)
}
