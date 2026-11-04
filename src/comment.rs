//! One short comment per change: which journeys now behave differently, which of those nobody
//! asked for, and whether an earlier decision was touched. Answers are kept as lines in a plain
//! `decisions.md`, a file people and agents can read and merge. No AI: the same change, request and
//! decisions always give the same comment.

use crate::change::{compare, JState, Report};
use crate::source::changed_paths;
use crate::util::{fnv64, human};
use crate::vcs::Vcs;
use crate::words::{change_key, evidence, grounded, Words};
use crate::Lift;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub const FILE: &str = "decisions.md";

const SHOWN: usize = 3;
const FACTS: usize = 2;

/// A journey the change altered.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub state: JState,
    /// Plain facts, as "step: what is different".
    pub facts: Vec<String>,
    /// Names of the steps this change altered.
    pub steps: Vec<String>,
    pub weight: usize,
    /// Identifies this exact change to this journey.
    pub key: String,
    /// A checked plain sentence, when one is saved. The facts stand in otherwise.
    pub sentence: Option<String>,
    /// What it did before, as a request, when one is saved.
    pub undo: Option<String>,
    /// Other journeys the very same change reaches. One change, one question.
    pub also: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Comment {
    pub asked: Option<String>,
    pub not_asked: Vec<Item>,
    /// What was asked for or, with no request to compare, everything, most significant first.
    pub changed: Vec<Item>,
    /// Everything too small to show: changes outside journeys, files, packages.
    pub smaller: Vec<String>,
    /// Earlier decisions this change alters.
    pub touched: Vec<String>,
    /// Decisions written by hand that share words with the change. They may not be related.
    pub may_touch: Vec<String>,
    /// Decisions deleted by this change.
    pub removed: Vec<String>,
    /// Journeys whose question was already answered, for exactly this behaviour.
    pub decided: Vec<String>,
    pub untouched: usize,
    /// The base to repeat in `flow decide`, when it is not HEAD.
    pub base: Option<String>,
}

impl Comment {
    pub fn is_empty(&self) -> bool {
        self.not_asked.is_empty() && self.changed.is_empty() && self.smaller.is_empty() && self.removed.is_empty()
    }
}

/// A line of `decisions.md`. `journey` and `mark` are absent on lines people wrote by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub text: String,
    pub journey: Option<String>,
    pub mark: Option<String>,
}

pub fn parse(text: &str) -> Vec<Decision> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*[-*]\s+(.*?)\s*(?:<!--\s*flow:\s*(.*)\|\s*(\S+)\s*-->\s*)?$").unwrap());
    text.lines()
        .filter_map(|l| re.captures(l))
        .map(|c| Decision { text: c[1].to_string(), journey: c.get(2).map(|m| m.as_str().trim().to_string()), mark: c.get(3).map(|m| m.as_str().to_string()) })
        .filter(|d| !d.text.is_empty())
        .collect()
}

/// A journey's steps, without the dispatcher every command of a CLI shares.
pub fn own_steps(l: &Lift, fi: usize) -> impl Iterator<Item = &crate::flows::Step> {
    let f = &l.flows[fi];
    f.steps.iter().filter(move |s| !(f.kind == "command" && s.depth == 0))
}

/// What a journey does now, by behaviour: reformatting leaves it the same.
pub fn fingerprint(l: &Lift, id: &str) -> String {
    l.journey(id).map(|fi| fingerprint_of(l, fi)).unwrap_or_else(|| "gone".into())
}

/// The same for a journey already found, which is how the far side of a rename is reached.
pub fn fingerprint_of(l: &Lift, fi: usize) -> String {
    let at = own_steps(l, fi).map(|s| s.at).chain(crate::change::constants(l, fi));
    // By permanent id, so moving a function does not read as changing what the journey does.
    let mut v: Vec<String> = at.map(|r| format!("{}={}", l.uids.step(&l.repo.sid(r)), l.repo.slice(r).shape)).collect();
    v.sort();
    fnv64(&v.join("\n"))
}

fn stem(w: &str) -> String {
    let mut s = w.to_string();
    for end in ["ing", "ed", "es", "s"] {
        if s.len() > end.len() + 2 && s.ends_with(end) && !(end == "s" && s.ends_with("ss")) {
            s.truncate(s.len() - end.len());
            break;
        }
    }
    if s.len() > 3 && s.ends_with('e') {
        s.pop();
    }
    let b = s.as_bytes();
    if b.len() > 3 && b[b.len() - 1] == b[b.len() - 2] {
        s.pop();
    }
    s
}

/// The words that can tell one journey from another, reduced so "upgrades" meets "upgrade".
pub fn words(text: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "from", "that", "this", "when", "then", "than", "should", "must", "make", "into", "not", "are", "was", "will", "can", "add", "fix", "all", "new", "now", "use", "get",
        "post", "put", "patch", "delete", "api", "app", "lib", "src", "page", "route", "has", "have", "does", "their", "they", "them", "our", "you", "your", "out", "off", "but", "only", "also",
    ];
    let clean: String = text.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    clean.split_whitespace().flat_map(|w| human(w).to_lowercase().split_whitespace().map(str::to_string).collect::<Vec<_>>()).filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit())).map(|w| stem(&w)).collect()
}

/// Split what changed into what was asked for and what was not.
///
/// A journey named by the request counts as asked. Words most journeys share ("customer") say
/// nothing and do not count. When the request names any journey, the others are side effects, even
/// if they run a step it mentions: that is the surprise this exists to catch. Only a request that
/// names no journey at all is matched against the steps that changed.
pub fn split(asked: &str, items: Vec<Item>, journeys: &[String]) -> (Vec<Item>, Vec<Item>) {
    let want = words(asked);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for j in journeys {
        for w in words(j) {
            *seen.entry(w).or_default() += 1;
        }
    }
    let common = (journeys.len() / 4).max(2);
    let useful: BTreeSet<&String> = want.iter().filter(|w| seen.get(*w).copied().unwrap_or(0) <= common).collect();
    let title_words = |i: &Item| words(&format!("{} {}", i.title, i.id));
    // "annual upgrades" against "Customer upgrades annual plan" and "Customer cancels annual plan":
    // the word fewest journeys have says which one was meant.
    let rarest = items.iter().flat_map(|i| title_words(i).into_iter().filter(|w| useful.contains(w)).collect::<Vec<_>>()).map(|w| seen.get(&w).copied().unwrap_or(0)).min();
    let by_title = |i: &Item| title_words(i).iter().any(|w| useful.contains(w) && Some(seen.get(w).copied().unwrap_or(0)) == rarest);
    if items.iter().any(by_title) {
        return items.into_iter().partition(|i| by_title(i));
    }
    items.into_iter().partition(|i| i.steps.iter().any(|s| words(s).iter().any(|w| useful.contains(w))))
}

pub fn items(r: &Report) -> Vec<Item> {
    let mut v: Vec<Item> = r
        .journeys
        .iter()
        .map(|j| {
            let mut facts = vec![];
            for f in r.functions.iter().filter(|f| f.journeys.contains(&j.title) && f.state != "reformatted only") {
                // A route's own handler has no name worth repeating: the journey already says it.
                let of = if j.id.split_whitespace().next() == Some(f.label.as_str()) { String::new() } else { format!("{}: ", f.label) };
                if f.facts.is_empty() {
                    facts.push(format!("{of}{}", f.state));
                }
                facts.extend(f.facts.iter().map(|x| format!("{of}{x}")));
            }
            let weight = j.edited.len() + j.added.len() + j.removed.len() + 2 * (j.now_changes.len() + j.no_longer_changes.len()) + if j.state == JState::Changed { 0 } else { 3 };
            if let Some(w) = &j.was {
                facts.insert(0, format!("renamed from {w}"));
            }
            Item { id: j.id.clone(), title: j.title.clone(), state: j.state.clone(), facts, steps: j.edited.iter().chain(&j.added).chain(&j.removed).cloned().collect(), weight, key: String::new(), sentence: None, undo: None, also: vec![] }
        })
        .collect();
    v.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.title.cmp(&b.title)));
    v
}

/// Everything outside journeys, so nothing hides.
fn smaller(r: &Report) -> Vec<String> {
    let mut v = vec![];
    for f in r.functions.iter().filter(|f| f.journeys.is_empty() && f.state != "reformatted only") {
        v.push(format!("{} ({}), no journey runs it", f.label, f.state));
    }
    for u in &r.new_unplaced {
        v.push(format!("new code no journey owns: {u}"));
    }
    for p in &r.packages {
        let to = |x: &Option<String>| x.clone().unwrap_or_else(|| "none".into());
        v.push(format!("package {}: {} -> {}", p.name, to(&p.from), to(&p.to)));
    }
    for s in &r.setup_changed {
        v.push(format!("imports or setup: {s}"));
    }
    for f in &r.other_files {
        match r.data_facts.iter().find(|d| &d.0 == f) {
            Some((_, facts)) => v.push(format!("{f}: {}", facts.iter().take(FACTS).cloned().collect::<Vec<_>>().join("; "))),
            None => v.push(f.clone()),
        }
    }
    v
}

/// Tie each item to its change and to anything saved about it.
pub fn annotate(items: &mut [Item], before: &Lift, after: &Lift, _words: &Words) {
    for i in items.iter_mut() {
        i.key = change_key(before, after, &i.id);
    }
}

/// Journeys changed in exactly the same way are one change that reaches further.
pub fn group(list: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = vec![];
    for i in list {
        match out.iter_mut().find(|o| !i.facts.is_empty() && o.facts == i.facts && o.state == i.state) {
            Some(o) => o.also.push(i.title),
            None => out.push(i),
        }
    }
    out
}

pub fn text_of(l: &Lift, path: &str) -> String {
    l.repo.file_idx(path).map(|i| l.repo.files[i].src.clone()).unwrap_or_default()
}

/// Lift both ends of a change and compare them. With no `head`, the end is the working folder.
pub fn between<R>(v: &mut Vcs, base: &str, head: Option<&str>, f: impl FnOnce(&Lift, &Lift, Report) -> R) -> Result<R> {
    let b = v.resolve(base)?;
    v.lift_rev(&b)?;
    let h = head.map(|h| v.resolve(h)).transpose()?;
    let (now, others) = match &h {
        Some(h) => {
            v.lift_rev(h)?;
            (None, v.other(changed_paths(&v.root, &b, h)?))
        }
        // Compared with the last saved snapshot, so a rename in progress already reads as one.
        None => (Some(v.now()?.0), v.changed_since(&b)?),
    };
    let before = v.cached(&b).context("lifted above")?;
    let after = match (&now, &h) {
        (Some(n), _) => n,
        (None, Some(h)) => v.cached(h).context("lifted above")?,
        (None, None) => unreachable!(),
    };
    let mut r = compare(before, after, &v.names, others);
    let ours = |p: &String| p == FILE || p == crate::words::FILE;
    r.other_files.retain(|p| !ours(p));
    r.data_facts.retain(|d| !ours(&d.0));
    Ok(f(before, after, r))
}

/// Lines tied to a journey, and lines written by hand.
pub fn split_decisions(d: Vec<Decision>) -> (Vec<Decision>, Vec<String>) {
    let (tied, hand): (Vec<Decision>, Vec<Decision>) = d.into_iter().partition(|x| x.journey.is_some());
    (tied, hand.into_iter().map(|x| x.text).collect())
}

pub fn write(before: &Lift, after: &Lift, r: &Report, asked: Option<String>, base: Option<String>) -> Comment {
    write_with(before, after, r, asked, base, &Words::parse(&text_of(after, crate::words::FILE)))
}

pub fn write_with(before: &Lift, after: &Lift, r: &Report, asked: Option<String>, base: Option<String>, saved: &Words) -> Comment {
    let mut all = items(r);
    for i in all.iter_mut() {
        i.key = change_key(before, after, &i.id);
        if let Some(e) = saved.says(&i.key) {
            if e.by == "person" || grounded(&e.sentence, &evidence(before, after, r, i)) {
                i.sentence = Some(e.sentence.clone()).filter(|s| !s.is_empty());
                i.undo = Some(e.undo.clone()).filter(|s| !s.is_empty());
            }
        }
        // When only part of a journey's change was asked for, the other part is what to show.
        if let Some(e) = saved.judged(&i.key, asked.as_deref()) {
            if e.asked == Some(false) && !e.unasked.is_empty() && grounded(&e.unasked, &evidence(before, after, r, i)) {
                i.sentence = Some(e.unasked.clone());
            }
        }
    }
    let titles: Vec<String> = after.flows.iter().map(|f| f.id.clone()).collect();
    let (mut changed, mut not_asked) = match &asked {
        Some(a) => split(a, all.clone(), &titles),
        None => (all.clone(), vec![]),
    };
    // A saved judgement knows words the code does not use, so it outranks matching.
    if asked.is_some() {
        let says = |i: &Item| saved.judged(&i.key, asked.as_deref()).and_then(|e| e.asked);
        let (yes, stay): (Vec<Item>, Vec<Item>) = not_asked.into_iter().partition(|i| says(i) == Some(true));
        let (no, keep): (Vec<Item>, Vec<Item>) = changed.into_iter().partition(|i| says(i) == Some(false));
        not_asked = stay.into_iter().chain(no).collect();
        changed = keep.into_iter().chain(yes).collect();
        not_asked.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.title.cmp(&b.title)));
    }

    let (old, new) = (parse(&text_of(before, FILE)), parse(&text_of(after, FILE)));
    let mut c = Comment { asked, untouched: r.untouched, base, ..Default::default() };
    c.removed = old.iter().filter(|d| !new.iter().any(|n| n.text == d.text)).map(|d| d.text.clone()).collect();

    // Words on the steps of most journeys say nothing about which decision this is.
    let mut spread: BTreeMap<String, usize> = BTreeMap::new();
    for (fi, _) in after.flows.iter().enumerate() {
        let have: BTreeSet<String> = own_steps(after, fi).flat_map(|s| words(&after.repo.slice(s.at).name)).collect();
        for w in have {
            *spread.entry(w).or_default() += 1;
        }
    }
    let wide = (after.flows.len() / 2).max(2);
    let shares_words = |d: &Decision, i: &Item| {
        let have = words(&format!("{} {} {}", i.title, i.steps.join(" "), i.facts.join(" ")));
        words(&d.text).intersection(&have).filter(|w| spread.get(*w).copied().unwrap_or(0) <= wide).count() >= 2
    };
    let judged: Vec<&crate::words::Entry> = all.iter().filter_map(|i| saved.judged(&i.key, c.asked.as_deref())).collect();
    for d in &new {
        match &d.journey {
            Some(j) => {
                // A decision stays with its journey through a rename.
                let j = after.journey(j).map(|fi| after.flows[fi].id.as_str()).unwrap_or(j);
                let Some(i) = all.iter().find(|i| i.id == j) else { continue };
                if d.mark.as_deref() == Some(fingerprint(after, j).as_str()) {
                    c.decided.push(i.title.clone());
                } else {
                    c.touched.push(d.text.clone());
                }
            }
            None => {
                if judged.len() == all.len() && !all.is_empty() {
                    if judged.iter().any(|e| e.touches.contains(&d.text)) {
                        c.touched.push(d.text.clone());
                    }
                } else if all.iter().any(|i| shares_words(d, i)) {
                    c.may_touch.push(d.text.clone());
                }
            }
        }
    }
    c.decided.sort();
    c.decided.dedup();
    c.touched.dedup();

    // An answered question is never asked twice.
    let (answered, open): (Vec<Item>, Vec<Item>) = not_asked.drain(..).partition(|i| c.decided.contains(&i.title));
    not_asked = open;
    changed.extend(answered);
    changed.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.title.cmp(&b.title)));

    c.not_asked = group(not_asked);
    c.changed = group(changed);
    c.smaller = smaller(r);
    c
}

fn cut(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s
    }
}

/// The line that heads an item: its sentence, or the journey when there is none.
fn head(i: &Item) -> String {
    i.sentence.clone().unwrap_or_else(|| format!("{}{}", i.title, state_word(i)))
}

/// How far a shared change reaches, when it reaches more than one journey.
fn reach(i: &Item) -> Option<String> {
    if i.also.is_empty() {
        return None;
    }
    let more = i.also.len() - i.also.len().min(2);
    let named = i.also.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
    Some(format!("reaches {} journeys: {}, {named}{}", i.also.len() + 1, i.title, if more > 0 { format!(" and {more} more") } else { String::new() }))
}

fn state_word(i: &Item) -> &'static str {
    match i.state {
        JState::New => " (new)",
        JState::Gone => " (removed)",
        JState::Changed => "",
    }
}

fn shown_facts(i: &Item, full: bool) -> Vec<String> {
    if i.sentence.is_some() && !full {
        return vec![];
    }
    let mut v: Vec<String> = i.facts.iter().take(FACTS).map(|f| cut(f, 90)).collect();
    if i.facts.len() > FACTS {
        v.push(format!("and {} more", i.facts.len() - FACTS));
    }
    v
}

/// The two answers to a question, as commands.
fn answers(c: &Comment, i: &Item) -> (String, String) {
    let base = c.base.as_ref().map(|b| format!(" --base {b}")).unwrap_or_default();
    let no = i.undo.clone().unwrap_or_else(|| "what it should do".into());
    (format!("flow decide {:?} yes{base}", i.id), format!("flow decide {:?} no {no:?}{base}", i.id))
}

/// Whatever did not fit in the first lines, in full.
fn folded(c: &Comment) -> Vec<String> {
    let mut v: Vec<String> = c.not_asked.iter().skip(SHOWN).map(|i| format!("Not asked: {}", head(i))).collect();
    v.extend(c.changed.iter().skip(SHOWN).map(head));
    v.extend(c.smaller.iter().cloned());
    v
}

/// Plain text, for the terminal.
pub fn render(c: &Comment, full: bool) -> String {
    if c.is_empty() {
        return "Nothing changed.\n".into();
    }
    let mut o = String::new();
    if let Some(a) = &c.asked {
        o += &format!("Asked:     {}\n", cut(a, 76));
    }
    for t in &c.removed {
        o += &format!("Removed:   this change deletes a decision: {t}\n");
    }
    for (n, i) in c.not_asked.iter().take(SHOWN).enumerate() {
        o += &format!("{}{}\n", if n == 0 { "Not asked: " } else { "           " }, head(i));
        if let Some(r) = reach(i) {
            o += &format!("             {r}\n");
        }
        for f in shown_facts(i, full) {
            o += &format!("             {f}\n");
        }
        o += "           Is that what you want?\n";
        let (yes, no) = answers(c, i);
        o += &format!("             {yes}   or   {no}\n");
    }
    for (n, i) in c.changed.iter().take(SHOWN).enumerate() {
        o += &format!("{}{}\n", if n == 0 { "Changed:   " } else { "           " }, head(i));
        if let Some(r) = reach(i) {
            o += &format!("             {r}\n");
        }
        for f in shown_facts(i, full) {
            o += &format!("             {f}\n");
        }
    }
    let rest = folded(c);
    if full {
        for r in &rest {
            o += &format!("           {r}\n");
        }
    } else if !rest.is_empty() {
        o += &format!("           + {} smaller {} (--full)\n", rest.len(), if rest.len() == 1 { "change" } else { "changes" });
    }
    for d in &c.decided {
        o += &format!("Decided:   {d} was already answered for this behaviour\n");
    }
    if c.touched.is_empty() && c.may_touch.is_empty() {
        o += "Kept:      no earlier decision was touched\n";
    }
    for (n, t) in c.touched.iter().enumerate() {
        o += &format!("{}{t}\n", if n == 0 { "Touched:   " } else { "           " });
    }
    for (n, t) in c.may_touch.iter().enumerate() {
        o += &format!("{}{t}\n", if n == 0 { "May touch: " } else { "           " });
    }
    o
}

/// Markdown, for a pull request. The answers run in a checkout, so they stay as commands.
pub fn markdown(c: &Comment) -> String {
    if c.is_empty() {
        return "Nothing changed.\n".into();
    }
    let esc = |s: &str| s.replace('`', "'").replace('<', "&lt;");
    let mut o = String::new();
    if let Some(a) = &c.asked {
        o += &format!("**Asked:** {}\n\n", esc(&cut(a, 200)));
    }
    for t in &c.removed {
        o += &format!("**Removed:** this change deletes a decision: {}\n\n", esc(t));
    }
    for i in c.not_asked.iter().take(SHOWN) {
        o += &format!("**Not asked:** {}\n", esc(&head(i)));
        if let Some(r) = reach(i) {
            o += &format!("\n{}\n", esc(&r));
        }
        for f in shown_facts(i, false) {
            o += &format!("- `{}`\n", esc(&f));
        }
        o += &format!("\nIs that what you want?\n- [ ] Yes, keep it <!-- flow-yes: {} -->\n", i.id);
        match &i.undo {
            Some(u) => o += &format!("- [ ] No, {} <!-- flow-no: {} -->\n", esc(u), i.id),
            None => o += &format!("\nOr reply `/flow no {}: what it should do`\n", i.id),
        }
        o += "\n";
    }
    let shown: Vec<&Item> = c.changed.iter().take(SHOWN).collect();
    if !shown.is_empty() {
        o += "**Changed:**\n";
    }
    for i in shown {
        o += &format!("- {}", esc(&head(i)));
        if let Some(r) = reach(i) {
            o += &format!("\n  - {}", esc(&r));
        }
        for f in shown_facts(i, false) {
            o += &format!("\n  - `{}`", esc(&f));
        }
        o += "\n";
    }
    let rest = folded(c);
    if !rest.is_empty() {
        o += &format!("\n<details><summary>+ {} smaller {}</summary>\n\n", rest.len(), if rest.len() == 1 { "change" } else { "changes" });
        for r in &rest {
            o += &format!("- {}\n", esc(r));
        }
        o += "\n</details>\n";
    }
    o += "\n";
    for d in &c.decided {
        o += &format!("**Decided:** {} was already answered for this behaviour\n\n", esc(d));
    }
    if c.touched.is_empty() && c.may_touch.is_empty() {
        o += "**Kept:** no earlier decision was touched\n";
    }
    for t in &c.touched {
        o += &format!("**Touched:** {}\n\n", esc(t));
    }
    for t in &c.may_touch {
        o += &format!("**May touch:** {}\n\n", esc(t));
    }
    o
}

/// The request, from the flag, a file, or the pull request GitHub says this run is for.
pub fn asked_text(asked: Option<String>, file: Option<std::path::PathBuf>) -> Result<Option<String>> {
    let text = match (asked, file) {
        (Some(a), _) => Some(a),
        (None, Some(p)) => Some(std::fs::read_to_string(&p).with_context(|| format!("could not read {}", p.display()))?),
        (None, None) => std::env::var("GITHUB_EVENT_PATH").ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v["pull_request"]["body"].as_str().or(v["issue"]["body"].as_str()).map(str::to_string)),
    };
    Ok(text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    // Days to a civil date, after Howard Hinnant's algorithm.
    let z = secs / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

fn find_journey(id: &str, l: &Lift, names: &crate::names::Names) -> Vec<String> {
    let want = id.to_lowercase();
    let title = |f: &crate::flows::Flow| crate::change::journey_title(names, &f.id).to_lowercase();
    let exact: Vec<String> = l.flows.iter().filter(|f| f.id.to_lowercase() == want || title(f) == want).map(|f| f.id.clone()).collect();
    if !exact.is_empty() {
        return exact;
    }
    l.flows.iter().filter(|f| f.id.to_lowercase().contains(&want) || title(f).contains(&want)).map(|f| f.id.clone()).collect()
}

/// Record an answer as one line of `decisions.md`. "yes" keeps what the journey does now, and the
/// question is not asked again until it does something else. "no" says what it should do, and
/// stays open on every change to that journey. The same answer twice is one line.
pub fn decide(v: &mut Vcs, journey: &str, answer: &str, sentence: Option<&str>, base: &str) -> Result<String> {
    let yes = match answer {
        "yes" => true,
        "no" => false,
        _ => bail!("the answer is yes or no"),
    };
    let names = v.names.clone();
    let root = v.root.clone();
    let asked = asked_text(None, None)?.or_else(|| crate::agent::asked_so_far(v));
    let (id, item, mark, peers) = between(v, base, None, |a, b, r| -> Result<(String, Option<Item>, String, Vec<(String, String)>)> {
        let mut hits = find_journey(journey, b, &names);
        if hits.is_empty() {
            hits = find_journey(journey, a, &names);
        }
        match hits.len() {
            0 => bail!("no journey matches {journey:?}"),
            1 => {}
            _ => bail!("{journey:?} matches several journeys: {}", hits.join(", ")),
        }
        let id = hits.remove(0);
        let c = write_with(a, b, &r, asked, None, &Words::load(b, &root));
        // An answer covers the journeys the comment showed with this question, and no others.
        let all = items(&r);
        let shown: Vec<&Item> = c.not_asked.iter().chain(&c.changed).collect();
        let item = all.iter().find(|i| i.id == id).cloned().map(|mut i| {
            let mine = shown.iter().find(|x| x.id == i.id || x.also.contains(&i.title));
            i.sentence = mine.and_then(|x| x.sentence.clone());
            i.undo = mine.and_then(|x| x.undo.clone());
            i
        });
        let line = item.as_ref().and_then(|i| shown.iter().find(|x| x.id == i.id || x.also.contains(&i.title)));
        let same = |o: &&Item| item.as_ref().is_some_and(|i| o.id != i.id) && line.is_some_and(|x| x.id == o.id || x.also.contains(&o.title));
        let peers: Vec<(String, String)> = all.iter().filter(same).map(|o| (o.id.clone(), fingerprint(b, &o.id))).collect();
        let mark = fingerprint(b, &id);
        Ok((id, item, mark, peers))
    })??;
    let title = crate::change::journey_title(&names, &id);
    let undo = item.as_ref().and_then(|i| i.undo.clone());
    let facts = item.as_ref().map(|i| i.facts.clone()).unwrap_or_default();
    let told = item.as_ref().and_then(|i| i.sentence.clone());
    let said = match (sentence, yes) {
        (Some(s), _) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        (None, false) => match undo {
            Some(u) => format!("{title}: {u}"),
            None => bail!("say what it should do instead: flow decide {journey:?} no \"what it should do\""),
        },
        (None, true) => match told {
            Some(t) => t,
            None if facts.is_empty() => format!("{title} stays as it is"),
            None => format!("{title} is meant to work this way: {}", cut(&facts.join("; "), 160)),
        },
    };
    let path = v.root.join(FILE);
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| "# decisions.md\n".to_string());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    for (j, m) in std::iter::once((id, mark)).chain(peers) {
        let tail = format!("<!-- flow: {j} | {} -->", if yes { m.as_str() } else { "open" });
        if !text.lines().any(|l| l.ends_with(&tail) && l.contains(&said)) {
            text += &format!("- {said} ({}) {tail}\n", today());
        }
    }
    std::fs::write(&path, text)?;
    Ok(said)
}

/// Answers given on a pull request: ticked boxes, and replies like `/flow no <journey>: <sentence>`.
pub fn answers_in(text: &str) -> Vec<(String, String, Option<String>)> {
    static TICK: OnceLock<Regex> = OnceLock::new();
    static REPLY: OnceLock<Regex> = OnceLock::new();
    let tick = TICK.get_or_init(|| Regex::new(r"^\s*[-*]\s+\[[xX]\]\s.*<!--\s*flow-(yes|no):\s*(.*?)\s*-->\s*$").unwrap());
    let reply = REPLY.get_or_init(|| Regex::new(r"^\s*/flow\s+(yes|no)\s+(.+?)(?::\s+(.+?))?\s*$").unwrap());
    let mut out = vec![];
    for l in text.lines() {
        if let Some(c) = tick.captures(l) {
            out.push((c[2].to_string(), c[1].to_string(), None));
        } else if let Some(c) = reply.captures(l) {
            out.push((c[2].to_string(), c[1].to_string(), c.get(3).map(|m| m.as_str().to_string())));
        }
    }
    out
}

/// Record every answer in a comment. Ticking both boxes for one journey is not an answer.
pub fn answer(v: &mut Vcs, text: &str, base: &str) -> Result<Vec<String>> {
    let found = answers_in(text);
    let mut out = vec![];
    for (id, a, s) in &found {
        if found.iter().any(|(i, b, _)| i == id && b != a) {
            bail!("{id} is answered both yes and no");
        }
        out.push(decide(v, id, a, s.as_deref(), base)?);
    }
    Ok(out)
}
