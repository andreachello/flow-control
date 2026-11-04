//! What people come to do. A flow is one request: a page, an action, a route. A journey is the goal
//! those requests add up to, like "Add a house", which takes a page, a form and a save.
//!
//! The code shows which requests lead to which, but not what anyone is trying to do, so Claude
//! proposes the journeys and they are kept in a committed file for a person to correct. Nothing is
//! asked when a page loads, and the lifter never reads this file: a journey only groups what the
//! lift already found. A request stays listed after its code is gone, so old changes still read
//! as the journey they belonged to.
//!
//! Each journey is also written out as its stops: what a person does, in order, and the requests
//! behind each one. That is what lets a change be shown as the place in the journey it reaches.

use crate::ids::Ids;
use crate::names::{doc_of, parallel, run_claude, STYLE};
use crate::Lift;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub const FILE: &str = "flow.journeys.json";
pub const ICONS: &[&str] = &[
    "enter", "plus", "cross", "up", "spark", "plug", "home", "calendar", "chat", "card", "search", "key", "star", "photo", "mail", "chart", "globe", "people", "doc", "pin", "gear",
];
/// The icon of the journey nobody starts: cron jobs, webhooks and the like.
pub const BEHIND: &str = "gear";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Journey {
    pub title: String,
    pub icon: String,
    pub requests: Vec<String>,
    /// What a person does along the way, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stops: Vec<Stop>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub say: String,
    pub requests: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct Journeys {
    #[serde(default)]
    pub journeys: Vec<Journey>,
}

/// Which journey a request belongs to, through any rename since the file was written.
#[derive(Default)]
pub struct Places {
    by_uid: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
}

impl Places {
    pub fn of(&self, uid: &str, name: &str) -> Option<usize> {
        self.by_uid.get(uid).or_else(|| self.by_name.get(name)).copied()
    }
}

impl Journeys {
    pub fn load(p: &Path) -> Result<Journeys> {
        if !p.exists() {
            return Ok(Journeys::default());
        }
        serde_json::from_str(&std::fs::read_to_string(p)?).with_context(|| format!("reading {}", p.display()))
    }

    pub fn save(&self, p: &Path) -> Result<()> {
        std::fs::write(p, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    pub fn places(&self, ids: &Ids) -> Places {
        let mut out = Places::default();
        for (i, j) in self.journeys.iter().enumerate() {
            for r in &j.requests {
                out.by_uid.entry(ids.journey(r)).or_insert(i);
                out.by_name.entry(r.clone()).or_insert(i);
            }
        }
        out
    }

    pub fn placed(&self) -> BTreeSet<&str> {
        self.journeys.iter().flat_map(|j| j.requests.iter().map(String::as_str)).collect()
    }
}

/// Which journeys a change is about, given the journeys each changed function reaches: those of
/// its narrowest code. A fix to one page and to a shared layout is about that page. A change to
/// shared code alone is about everything that code reaches.
pub fn focus(reach: &[BTreeSet<usize>]) -> BTreeSet<usize> {
    let narrowest = reach.iter().map(BTreeSet::len).filter(|n| *n > 0).min();
    reach.iter().filter(|r| Some(r.len()) == narrowest).flatten().copied().collect()
}

/// The requests that have no journey yet.
pub fn unplaced<'a>(l: &'a Lift, have: &Journeys, ids: &Ids) -> Vec<&'a str> {
    let places = have.places(ids);
    l.flows.iter().filter(|f| places.of(&f.uid, &f.id).is_none()).map(|f| f.id.as_str()).collect()
}

/// What each request is, and where it leads, which is most of what says what belongs together.
fn described(l: &Lift, which: &[&str]) -> String {
    let entries: BTreeMap<_, &str> = l.flows.iter().map(|f| (f.entry, f.id.as_str())).collect();
    let mut p = String::new();
    for f in l.flows.iter().filter(|f| which.contains(&f.id.as_str())) {
        p += &format!("## {}\nkind: {}\n", f.id, f.kind);
        let d = doc_of(l, f.entry);
        if !d.is_empty() {
            p += &format!("comment: {}\n", crate::util::short(&d, 160));
        }
        let leads: BTreeSet<&str> = f.steps.iter().skip(1).filter_map(|s| entries.get(&s.at).copied()).collect();
        if !leads.is_empty() {
            p += &format!("leads to: {}\n", leads.into_iter().collect::<Vec<_>>().join(", "));
        }
        let w: BTreeSet<String> = f.steps.iter().flat_map(|s| l.graph.writes.get(&s.at).cloned().unwrap_or_default()).map(|k| k.rsplit('#').next().unwrap_or(&k).to_string()).collect();
        if !w.is_empty() {
            p += &format!("changes: {}\n", w.into_iter().take(12).collect::<Vec<_>>().join(", "));
        }
        p += "\n";
    }
    p
}

pub fn prompt(l: &Lift, have: &Journeys, todo: &[&str]) -> String {
    let rules = format!(
        "A journey is one thing a person comes to do, from start to finish, like \"Log in\", \"Upgrade to Pro\" or \"Cancel plan\". It is not a screen, and it is not a request. One journey usually takes several requests: the page they look at and the actions they take on it.\n\n{STYLE}\n\n\
         title: 1 to 3 words, starting with a verb, in the words of the person using the product.\n\
         icon: one of {}.\n\
         requests: the requests exactly as written below. Every request goes in exactly one journey.\n",
        ICONS.join(", ")
    );
    if have.journeys.is_empty() {
        return format!(
            "These are the requests of one product, read from its code. Group them into the journeys people take.\n\n{rules}\
             Use at most 6 journeys for the whole product, counting the last one below, so all of them fit on one line, and order them as a person meets them. Requests no person starts, like cron jobs, webhooks, sitemaps and middleware, go in one last journey called \"Behind the scenes\" with the icon {BEHIND}.\n\n{}",
            described(l, todo)
        );
    }
    let known: Vec<String> = have.journeys.iter().map(|j| format!("- {} (icon {}): {}", j.title, j.icon, j.requests.iter().take(8).cloned().collect::<Vec<_>>().join(", "))).collect();
    format!(
        "This product already has these journeys, shown with some of their requests:\n{}\n\nPut each new request below in one of them, using its exact title. Start a new journey only when none fits.\n\n{rules}\n{}",
        known.join("\n"),
        described(l, todo)
    )
}

pub fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["journeys"],
        "properties": {"journeys": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["title", "icon", "requests"],
            "properties": {"title": {"type": "string"}, "icon": {"type": "string", "enum": ICONS}, "requests": {"type": "array", "items": {"type": "string"}}}
        }}}
    })
}

/// Take in an answer. Journeys and their titles stay as they are, so colours and links hold:
/// only requests that have no journey are added, and only requests that exist are believed.
/// Returns how many were placed.
pub fn apply(have: &mut Journeys, answer: &Value, todo: &[&str]) -> usize {
    let mut open: BTreeSet<&str> = todo.iter().copied().collect();
    let mut placed = 0;
    for a in answer["journeys"].as_array().into_iter().flatten() {
        let title = a["title"].as_str().unwrap_or("").trim();
        let mine: Vec<String> = a["requests"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|r| open.remove(r)).map(str::to_string).collect();
        if title.is_empty() || mine.is_empty() {
            continue;
        }
        placed += mine.len();
        match have.journeys.iter_mut().find(|j| j.title.eq_ignore_ascii_case(title)) {
            Some(j) => j.requests.extend(mine),
            None => {
                let icon = a["icon"].as_str().filter(|i| ICONS.contains(i)).unwrap_or("star").to_string();
                have.journeys.push(Journey { title: title.to_string(), icon, requests: mine, stops: vec![] });
            }
        }
    }
    placed
}

pub fn stops_prompt(l: &Lift, j: &Journey) -> String {
    let which: Vec<&str> = j.requests.iter().map(String::as_str).collect();
    format!(
        "\"{}\" is one journey people take through a product. Below are the requests it is made of, read from the code. Write out the journey as its stops: what a person does, from start to finish, in the order they do it.\n\n{STYLE}\n\n\
         say: one short sentence at most 60 characters, starting with what the person does, ending in a full stop, like \"Pick a plan.\", \"Enter your card details.\" or \"Get a confirmation email.\"\n\
         requests: the requests behind this stop, exactly as written below. A request can sit under more than one stop only when a person really meets it twice.\n\
         Use 3 to 7 stops, and cover the whole journey as a person lives it, not every request. Group small steps into one stop. A journey with several separate tasks takes its stops in the order a person usually meets them.\n\n{}",
        j.title,
        described(l, &which)
    )
}

pub fn stops_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["stops"],
        "properties": {"stops": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["say", "requests"],
            "properties": {"say": {"type": "string"}, "requests": {"type": "array", "items": {"type": "string"}}}
        }}}
    })
}

/// Take in a written journey. A stop has to rest on requests of this journey, and one stop is no
/// journey at all, so anything less leaves the journey unwritten. Returns the stops kept.
pub fn take_stops(j: &mut Journey, answer: &Value) -> usize {
    let mut stops = vec![];
    for a in answer["stops"].as_array().into_iter().flatten() {
        let say = a["say"].as_str().unwrap_or("").trim();
        let mine: Vec<String> = a["requests"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|r| j.requests.iter().any(|x| x == r)).map(str::to_string).collect();
        if !say.is_empty() && say.chars().count() <= 80 && !mine.is_empty() {
            stops.push(Stop { say: say.to_string(), requests: mine });
        }
    }
    if stops.len() < 2 {
        return 0;
    }
    j.stops = stops;
    j.stops.len()
}

/// Write out every journey people take that has no stops yet. Returns the journeys written, and
/// what went wrong with the others.
pub fn walk(l: &Lift, have: &mut Journeys, model: &str, jobs: usize) -> (usize, Vec<String>) {
    let todo: Vec<usize> = (0..have.journeys.len()).filter(|&i| have.journeys[i].stops.is_empty() && have.journeys[i].icon != BEHIND).collect();
    let asks: Vec<(usize, String)> = todo.into_iter().map(|i| (i, stops_prompt(l, &have.journeys[i]))).collect();
    let (mut written, mut failed) = (0, vec![]);
    for (i, answer) in parallel(jobs, asks, |_, (i, p)| (i, run_claude(model, &p, &stops_schema()))) {
        let title = have.journeys[i].title.clone();
        match answer {
            Ok(a) if take_stops(&mut have.journeys[i], &a) > 0 => written += 1,
            Ok(_) => failed.push(format!("{title}: the stops did not match its requests")),
            Err(e) => failed.push(format!("{title}: {e:#}")),
        }
    }
    (written, failed)
}

pub struct Report {
    pub placed: usize,
    pub left: Vec<String>,
}

/// Group the requests that have no journey yet. Asks nothing when there are none.
pub fn group(l: &Lift, have: &mut Journeys, ids: &Ids, model: &str) -> Result<Report> {
    let todo = unplaced(l, have, ids);
    if todo.is_empty() {
        return Ok(Report { placed: 0, left: vec![] });
    }
    let answer = run_claude(model, &prompt(l, have, &todo), &schema())?;
    let placed = apply(have, &answer, &todo);
    Ok(Report { placed, left: unplaced(l, have, ids).into_iter().map(str::to_string).collect() })
}
