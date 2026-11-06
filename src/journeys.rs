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
use std::sync::atomic::{AtomicUsize, Ordering};

pub const FILE: &str = "flow.journeys.json";
/// Marks a line of output as where a long task has got to, for a page showing it.
pub const STEP: &str = "step: ";

fn step(say: String) {
    println!("{STEP}{say}");
}
pub const ICONS: &[&str] = &[
    "enter", "plus", "cross", "up", "spark", "plug", "home", "calendar", "chat", "card", "search", "key", "star", "photo", "mail", "chart", "globe", "people", "doc", "pin", "gear",
];
/// The icon of the journey nobody starts: cron jobs, webhooks and the like.
pub const BEHIND: &str = "gear";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Journey {
    /// The product this journey is part of, when the repo holds several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
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
    /// What a repo with several products holds. A request belongs to the one whose folder its
    /// file is in, and one in no product's folder goes behind the scenes, so shared code never
    /// has to be listed here. Lives in this file because it groups what a lift found, like the
    /// journeys do: changing it must not change how any old commit reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub products: Vec<Product>,
    /// Folders nobody takes a journey through, like examples.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub journeys: Vec<Journey>,
    /// How the journeys fit together, for the people who take them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<Map>,
    /// The same, one for each product.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub maps: BTreeMap<String, Map>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Product {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Map {
    /// The product in one sentence, as the journeys add up to it.
    pub say: String,
    pub paths: Vec<Path_>,
}

/// One kind of person, and the journeys they go through in the order they usually do.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Path_ {
    pub who: String,
    pub journeys: Vec<String>,
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

    pub fn product_of(&self, path: &str) -> Option<&str> {
        self.products.iter().find(|p| inside(path, &p.path)).map(|p| p.name.as_str())
    }

    pub fn ignored(&self, path: &str) -> bool {
        self.ignore.iter().any(|d| inside(path, d))
    }

    /// One product's journeys, as if it were the only one.
    fn only(&self, product: &str) -> Journeys {
        let journeys = self.journeys.iter().filter(|j| j.product.as_deref() == Some(product)).cloned().collect();
        Journeys { journeys, map: self.maps.get(product).cloned(), ..Journeys::default() }
    }

    pub fn placed(&self) -> BTreeSet<&str> {
        self.journeys.iter().flat_map(|j| j.requests.iter().map(String::as_str)).collect()
    }
}

fn inside(path: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    path.strip_prefix(dir).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

/// Folders that hold an app without being a product.
const NOT_PRODUCTS: &[&str] = &["example", "examples", "template", "templates", "fixture", "fixtures", "test", "tests", "demo", "demos"];

/// Propose products for a repo that holds several apps: one for each, and the rest left out. Only
/// before the first journey, so a project grouped as one is never pulled apart. A person corrects
/// the file like they do the journeys.
pub fn seed(l: &Lift, have: &mut Journeys) -> bool {
    if !have.products.is_empty() || !have.journeys.is_empty() {
        return false;
    }
    let mut apps: Vec<&String> = l.repo.apps.iter().filter(|a| !a.is_empty()).collect();
    apps.sort();
    let (spare, real): (Vec<&String>, Vec<&String>) = apps.into_iter().partition(|a| a.split('/').any(|s| NOT_PRODUCTS.contains(&s)));
    let real: Vec<&String> = real.into_iter().filter(|a| l.flows.iter().any(|f| inside(&l.repo.files[f.entry.0].path, a))).collect();
    if real.len() < 2 {
        return false;
    }
    have.products = real.iter().map(|a| {
        let last = a.rsplit('/').next().unwrap_or(a);
        let mut name = last.chars();
        Product { name: name.next().map(|c| c.to_uppercase().chain(name).collect()).unwrap_or_default(), path: a.to_string() }
    }).collect();
    have.ignore = spare.into_iter().cloned().collect();
    true
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
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
    l.flows.iter().filter(|f| places.of(&f.uid, &f.id).is_none() && !have.ignored(&l.repo.files[f.entry.0].path)).map(|f| f.id.as_str()).collect()
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
    apply_to(have, None, answer, todo)
}

fn apply_to(have: &mut Journeys, product: Option<&str>, answer: &Value, todo: &[&str]) -> usize {
    let mut open: BTreeSet<&str> = todo.iter().copied().collect();
    let mut placed = 0;
    for a in answer["journeys"].as_array().into_iter().flatten() {
        let title = a["title"].as_str().unwrap_or("").trim();
        let mine: Vec<String> = a["requests"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|r| open.remove(r)).map(str::to_string).collect();
        if title.is_empty() || mine.is_empty() {
            continue;
        }
        placed += mine.len();
        match have.journeys.iter_mut().find(|j| j.product.as_deref() == product && j.title.eq_ignore_ascii_case(title)) {
            Some(j) => j.requests.extend(mine),
            None => {
                let icon = a["icon"].as_str().filter(|i| ICONS.contains(i)).unwrap_or("star").to_string();
                have.journeys.push(Journey { product: product.map(str::to_string), title: title.to_string(), icon, requests: mine, stops: vec![] });
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
         Use 3 to 7 stops, or fewer when the journey is that small, and cover the whole journey as a person lives it, not every request. Group small steps into one stop. A journey with several separate tasks takes its stops in the order a person usually meets them.\n\n{}",
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
/// journey at all unless it only takes one request, so anything less leaves the journey unwritten.
/// Returns the stops kept.
pub fn take_stops(j: &mut Journey, answer: &Value) -> usize {
    let mut stops = vec![];
    for a in answer["stops"].as_array().into_iter().flatten() {
        let say = a["say"].as_str().unwrap_or("").trim();
        let mine: Vec<String> = a["requests"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|r| j.requests.iter().any(|x| x == r)).map(str::to_string).collect();
        if !say.is_empty() && say.chars().count() <= 80 && !mine.is_empty() {
            stops.push(Stop { say: say.to_string(), requests: mine });
        }
    }
    if stops.len() < 2.min(j.requests.len()) || stops.is_empty() {
        return 0;
    }
    j.stops = stops;
    j.stops.len()
}

/// Why an answer gave a journey no stops, in numbers, so a failure can be understood.
pub fn stops_problem(j: &Journey, answer: &Value) -> String {
    let stops = answer["stops"].as_array().map_or(&[][..], Vec::as_slice);
    let long = stops.iter().filter(|a| a["say"].as_str().unwrap_or("").trim().chars().count() > 80).count();
    let empty = stops.iter().filter(|a| a["say"].as_str().unwrap_or("").trim().is_empty()).count();
    let lost = stops.iter().filter(|a| !a["requests"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|r| j.requests.iter().any(|x| x == r))).count();
    format!("{} stops came back: {long} too long, {empty} with no words, {lost} naming no request of this journey, and it takes {} good ones", stops.len(), 2.min(j.requests.len()).max(1))
}

/// Write out every journey people take that has no stops yet. Returns the journeys written, and
/// what went wrong with the others.
pub fn walk(l: &Lift, have: &mut Journeys, model: &str, jobs: usize) -> (usize, Vec<String>) {
    let todo: Vec<usize> = (0..have.journeys.len()).filter(|&i| have.journeys[i].stops.is_empty() && have.journeys[i].icon != BEHIND).collect();
    let asks: Vec<(usize, String)> = todo.into_iter().map(|i| (i, stops_prompt(l, &have.journeys[i]))).collect();
    let (mut written, mut failed) = (0, vec![]);
    let (total, done) = (asks.len(), AtomicUsize::new(0));
    if total > 0 {
        step(format!("Writing out {}", plural(total, "journey")));
    }
    for (i, answer) in parallel(jobs, asks, |_, (i, p)| {
        let answer = run_claude(model, &p, &stops_schema());
        step(format!("Writing out journeys, {} of {total} done", done.fetch_add(1, Ordering::Relaxed) + 1));
        (i, answer)
    }) {
        let title = have.journeys[i].title.clone();
        match answer {
            Ok(a) if take_stops(&mut have.journeys[i], &a) > 0 => written += 1,
            // Answers vary, so one that does not fit gets a second try.
            Ok(_) => match run_claude(model, &stops_prompt(l, &have.journeys[i]), &stops_schema()) {
                Ok(a) if take_stops(&mut have.journeys[i], &a) > 0 => written += 1,
                Ok(a) => failed.push(format!("{title}: {}", stops_problem(&have.journeys[i], &a))),
                Err(e) => failed.push(format!("{title}: {e:#}")),
            },
            Err(e) => failed.push(format!("{title}: {e:#}")),
        }
    }
    (written, failed)
}

pub struct Report {
    pub placed: usize,
    pub left: Vec<String>,
    /// Products that could not be grouped this time, and why.
    pub failed: Vec<String>,
}

/// Group the requests that have no journey yet. Asks nothing when there are none.
pub fn map_prompt(have: &Journeys) -> String {
    let list: String = have.journeys.iter().filter(|j| j.icon != BEHIND).map(|j| {
        let stops: Vec<&str> = j.stops.iter().map(|s| s.say.as_str()).collect();
        format!("{}: {}\n", j.title, stops.join(" "))
    }).collect();
    format!(
        "Below are the journeys of one product, each with the stops a person makes. Show how they fit together: who uses the product, and which journeys each kind of person goes through.\n\n{STYLE}\n\n\
         say: one sentence at most 120 characters on what the product lets these people do together, ending in a full stop.\n\
         paths: one per kind of person, at most 4.\n\
         who: that person in one or two plain words, plural, like \"Guests\" or \"Team admins\".\n\
         journeys: the journeys they go through, in the order they usually meet them, each title written exactly as below. A journey shared by two kinds of person goes in both paths. Every journey below must appear in at least one path.\n\n{list}"
    )
}

pub fn map_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["say", "paths"],
        "properties": {"say": {"type": "string"}, "paths": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["who", "journeys"],
            "properties": {"who": {"type": "string"}, "journeys": {"type": "array", "items": {"type": "string"}}}
        }}}
    })
}

/// Only a map that names real journeys and leaves none out. A partial one would mislead.
pub fn take_map(have: &mut Journeys, answer: &Value) -> bool {
    let say = answer["say"].as_str().unwrap_or("").trim().to_string();
    let known = |t: &str| have.journeys.iter().any(|j| j.title == t && j.icon != BEHIND);
    let mut paths = vec![];
    for p in answer["paths"].as_array().into_iter().flatten() {
        let who = p["who"].as_str().unwrap_or("").trim().to_string();
        let journeys: Vec<String> = p["journeys"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|t| known(t)).map(str::to_string).collect();
        if !who.is_empty() && !journeys.is_empty() {
            paths.push(Path_ { who, journeys });
        }
    }
    let whole = have.journeys.iter().filter(|j| j.icon != BEHIND).all(|j| paths.iter().any(|p| p.journeys.contains(&j.title)));
    if say.is_empty() || say.chars().count() > 160 || paths.is_empty() || !whole {
        return false;
    }
    have.map = Some(Map { say, paths });
    true
}

/// Drawn once, like everything here: delete `map` from the file to ask again.
pub fn draw(have: &mut Journeys, model: &str) -> Result<bool> {
    if have.products.is_empty() {
        return draw_one(have, model);
    }
    let names: Vec<String> = have.products.iter().map(|p| p.name.clone()).collect();
    let (mut drew, mut failed) = (false, vec![]);
    for name in names {
        let mut one = have.only(&name);
        step(format!("Drawing how {name} fits together"));
        match draw_one(&mut one, model) {
            Ok(true) => {
                have.maps.extend(one.map.map(|m| (name, m)));
                drew = true;
            }
            Ok(false) => {}
            Err(e) => failed.push(format!("{name}: {e:#}")),
        }
    }
    if failed.is_empty() {
        return Ok(drew);
    }
    anyhow::bail!("{}", failed.join("; "))
}

fn draw_one(have: &mut Journeys, model: &str) -> Result<bool> {
    if have.map.is_some() || have.journeys.iter().all(|j| j.icon == BEHIND) {
        return Ok(false);
    }
    let answer = run_claude(model, &map_prompt(have), &map_schema())?;
    if take_map(have, &answer) {
        return Ok(true);
    }
    anyhow::bail!("the map did not name every journey")
}

pub fn group(l: &Lift, have: &mut Journeys, ids: &Ids, model: &str) -> Result<Report> {
    seed(l, have);
    let todo = unplaced(l, have, ids);
    if todo.is_empty() {
        return Ok(Report { placed: 0, left: vec![], failed: vec![] });
    }
    if have.products.is_empty() {
        step(format!("Grouping {}", plural(todo.len(), "request")));
        let answer = run_claude(model, &prompt(l, have, &todo), &schema())?;
        let placed = apply(have, &answer, &todo);
        return Ok(Report { placed, left: unplaced(l, have, ids).into_iter().map(str::to_string).collect(), failed: vec![] });
    }
    let (mut mine, mut shared): (BTreeMap<String, Vec<&str>>, Vec<String>) = Default::default();
    for f in l.flows.iter().filter(|f| todo.contains(&f.id.as_str())) {
        match have.product_of(&l.repo.files[f.entry.0].path) {
            Some(p) => mine.entry(p.to_string()).or_default().push(f.id.as_str()),
            None => shared.push(f.id.clone()),
        }
    }
    let asks: Vec<(String, Vec<&str>, String)> = mine.into_iter().map(|(p, t)| {
        let ask = prompt(l, &have.only(&p), &t);
        (p, t, ask)
    }).collect();
    let (mut placed, mut failed) = (0, vec![]);
    let (total, done) = (asks.len(), AtomicUsize::new(0));
    step(format!("Grouping {} across {}", plural(todo.len(), "request"), plural(total, "product")));
    for (p, t, answer) in parallel(4, asks, |_, (p, t, ask)| {
        let answer = run_claude(model, &ask, &schema());
        step(format!("Grouped {p}, {} of {total} products done", done.fetch_add(1, Ordering::Relaxed) + 1));
        (p, t, answer)
    }) {
        match answer {
            Ok(a) => placed += apply_to(have, Some(&p), &a, &t),
            Err(e) => failed.push(format!("{p}: {e:#}")),
        }
    }
    // Code no product owns is not something a person sets out to do.
    if !shared.is_empty() {
        placed += shared.len();
        match have.journeys.iter_mut().find(|j| j.product.is_none() && j.icon == BEHIND) {
            Some(j) => j.requests.extend(shared),
            None => have.journeys.push(Journey { product: None, title: "Behind the scenes".into(), icon: BEHIND.into(), requests: shared, stops: vec![] }),
        }
    }
    Ok(Report { placed, left: unplaced(l, have, ids).into_iter().map(str::to_string).collect(), failed })
}

#[cfg(test)]
mod map_tests {
    use super::*;

    #[test]
    fn a_rejected_answer_says_what_was_wrong_with_it() {
        let j = Journey { product: None, title: "Pay".into(), icon: "card".into(), requests: vec!["POST /api/pay".into()], stops: vec![] };
        let a = json!({ "stops": [{ "say": "x".repeat(81), "requests": ["POST /api/pay"] }, { "say": "Pay.", "requests": ["pay"] }] });
        let said = stops_problem(&j, &a);
        assert!(said.starts_with("2 stops came back: 1 too long, 0 with no words, 1 naming no request"), "{said}");
        assert!(stops_problem(&j, &json!({})).starts_with("0 stops"));
    }

    #[test]
    fn a_journey_of_one_request_can_be_one_stop() {
        let mut j = Journey { product: None, title: "See results".into(), icon: "chart".into(), requests: vec!["page /results".into()], stops: vec![] };
        assert_eq!(take_stops(&mut j, &json!({ "stops": [{ "say": "See your results.", "requests": ["page /results"] }] })), 1);
        j.requests.push("page /more".into());
        j.stops.clear();
        assert_eq!(take_stops(&mut j, &json!({ "stops": [{ "say": "See your results.", "requests": ["page /results"] }] })), 0, "two requests still take two stops");
    }

    fn book() -> Journeys {
        let j = |t: &str, i: &str| Journey { product: None, title: t.into(), icon: i.into(), requests: vec![], stops: vec![] };
        Journeys { journeys: vec![j("Book a stay", "calendar"), j("List a home", "plus"), j("Behind the scenes", BEHIND)], ..Journeys::default() }
    }

    #[test]
    fn a_map_keeps_real_journeys_and_a_shared_one_in_both_paths() {
        let mut b = book();
        let a = json!({ "say": "Hosts list homes and guests book them.", "paths": [{ "who": "Guests", "journeys": ["Book a stay", "Invented"] }, { "who": "Hosts", "journeys": ["List a home", "Book a stay"] }] });
        assert!(take_map(&mut b, &a));
        let m = b.map.unwrap();
        assert_eq!(m.paths[0].journeys, ["Book a stay"], "a journey that does not exist is dropped");
        assert_eq!(m.paths[1].journeys, ["List a home", "Book a stay"], "the order is the person's");
    }

    #[test]
    fn a_map_that_leaves_a_journey_out_is_refused() {
        let mut b = book();
        let a = json!({ "say": "Guests book.", "paths": [{ "who": "Guests", "journeys": ["Book a stay"] }] });
        assert!(!take_map(&mut b, &a));
        assert!(b.map.is_none());
    }

    #[test]
    fn an_old_file_without_a_map_still_reads_and_saves_the_same() {
        let b: Journeys = serde_json::from_str(r#"{"journeys":[]}"#).unwrap();
        assert!(b.map.is_none());
        assert!(!serde_json::to_string(&b).unwrap().contains("map"));
    }
}
