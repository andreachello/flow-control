//! One short sentence for each change, and the few plain clauses behind it, in the words of the
//! people it reaches. A commit message is written for whoever reads the code; the list of changes
//! is read by everyone else. Each clause names the changed step it describes, so a reader can go
//! from the words to the code, and a clause about nothing that changed is thrown away.
//!
//! Claude writes them once and they are kept in a committed file, keyed by commit, for a person to
//! correct. Nothing is asked when a page loads, and a change without a title shows its message.

use crate::journeys::Journeys;
use crate::names::{parallel, run_claude, STYLE};
use crate::vcs::{Commit, Vcs};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const FILE: &str = "flow.titles.json";
/// What a change that reaches nobody says.
pub const QUIET: &str = "Nothing changes for people.";
const MOST: usize = 60;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub say: String,
    pub step: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Title {
    pub title: String,
    #[serde(default)]
    pub parts: Vec<Part>,
}

pub type Titles = BTreeMap<String, Title>;
/// Each commit asked about, and the steps it changed.
pub type Known = BTreeMap<String, BTreeSet<String>>;

pub fn load(p: &Path) -> Result<Titles> {
    if !p.exists() {
        return Ok(Titles::new());
    }
    serde_json::from_str(&std::fs::read_to_string(p)?).with_context(|| format!("reading {}", p.display()))
}

pub fn save(t: &Titles, p: &Path) -> Result<()> {
    std::fs::write(p, serde_json::to_string_pretty(t)? + "\n")?;
    Ok(())
}

/// A title has to fit one row, so anything else stays the commit message.
pub fn take(t: &mut Titles, known: &Known, answer: &Value) -> usize {
    let mut n = 0;
    for a in answer["titles"].as_array().into_iter().flatten() {
        let (id, title) = (a["id"].as_str().unwrap_or(""), a["title"].as_str().unwrap_or("").trim());
        let Some((sha, steps)) = known.iter().find(|(k, _)| !id.is_empty() && k.starts_with(id)) else { continue };
        if title.is_empty() || title.chars().count() > MOST {
            continue;
        }
        let parts = a["parts"].as_array().into_iter().flatten();
        let parts = parts.filter_map(|p| Some(Part { say: p["say"].as_str()?.trim().to_string(), step: p["step"].as_str()?.to_string() }));
        let parts = parts.filter(|p| !p.say.is_empty() && steps.contains(&p.step) && title != QUIET).collect();
        t.insert(sha.clone(), Title { title: title.to_string(), parts });
        n += 1;
    }
    n
}

fn described(v: &mut Vcs, c: &Commit, book: &Journeys, ids: &crate::ids::Ids) -> Result<(String, BTreeSet<String>)> {
    let r = v.change_of(c)?;
    let places = book.places(ids);
    let mut reached: Vec<&str> = r.journeys.iter().filter_map(|j| places.of(&j.uid, &j.id)).map(|g| book.journeys[g].title.as_str()).collect();
    reached.sort();
    reached.dedup();
    let code: Vec<_> = r.functions.iter().filter(|f| f.state != "reformatted only" && !crate::output::is_test_file(&f.sid)).take(24).collect();
    let steps: Vec<String> = code.iter().map(|f| format!("- {} ({}): {}", f.sid, f.label, crate::util::short(f.facts.first().unwrap_or(&f.state), 110))).collect();
    let text = format!("## {}\nmessage: {}\nreaches: {}\nfiles outside the code: {}\nchanged steps:\n{}\n\n", &c.id[..12], c.subject, reached.join(", "), r.other_files.iter().take(5).cloned().collect::<Vec<_>>().join(", "), steps.join("\n"));
    Ok((text, code.iter().map(|f| f.sid.clone()).collect()))
}

pub struct Report {
    pub titled: usize,
    pub failures: Vec<String>,
}

/// Title the newest `last` changes that have no title yet.
pub fn write(v: &mut Vcs, book: &Journeys, have: &mut Titles, last: usize, model: &str, jobs: usize) -> Result<Report> {
    let (ids, _) = crate::ids::Ids::parse(&std::fs::read_to_string(v.root.join(crate::ids::FILE)).unwrap_or_default());
    let todo: Vec<Commit> = v.commits_lifted(last, "HEAD")?.into_iter().filter(|c| !have.contains_key(&c.id)).collect();
    let mut batches = vec![];
    for chunk in todo.chunks(25) {
        let mut p = format!(
            "Each block below is one saved change to a product. Write its title for the list of changes.\n\n{STYLE}\n\n\
             title: one short sentence ending in a full stop, at most 48 characters, saying what is different for the person using the product, like \"Pay yearly for the price of ten months.\", \"Agent replies name their tools.\" or \"Everyone who pays gets a receipt.\" Say what changed, not that something was updated. Use the words of the product, not of the code.\n\
             When nobody using the product would notice, such as a script, a test or a tidy-up, the title is exactly \"{QUIET}\" and parts is empty.\n\
             parts: 1 to 7 short clauses that read in order as one flowing paragraph about what changed for people, each at most 40 characters, like \"People can pay yearly,\" then \"for ten months' price,\" then \"with unused days credited.\" Each clause has step: the id of the changed step it describes, copied exactly from that change's list. Only say what those steps show. Do not repeat the title.\n\
             id: exactly as given.\n\n"
        );
        let mut known = Known::new();
        for c in chunk {
            let (text, steps) = described(v, c, book, &ids)?;
            p += &text;
            known.insert(c.id.clone(), steps);
        }
        batches.push((known, p));
    }
    let schema = json!({
        "type": "object", "additionalProperties": false, "required": ["titles"],
        "properties": {"titles": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "title", "parts"],
            "properties": {"id": {"type": "string"}, "title": {"type": "string"}, "parts": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["say", "step"],
                "properties": {"say": {"type": "string"}, "step": {"type": "string"}}
            }}}
        }}}
    });
    let mut rep = Report { titled: 0, failures: vec![] };
    for (known, answer) in parallel(jobs, batches, |_, (known, p)| (known, run_claude(model, &p, &schema))) {
        match answer {
            Ok(a) => rep.titled += take(have, &known, &a),
            Err(e) => rep.failures.push(format!("{e:#}")),
        }
    }
    Ok(rep)
}
