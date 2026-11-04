//! The first product: one comment per change, and decisions that outlast it.

use flowcontrol::comment::{answer, answers_in, between, decide, markdown, parse, render, split, write, write_with, Comment, Item, FILE};
use flowcontrol::words::{fill, grounded, Words};
use serde_json::{json, Value};
use flowcontrol::change::JState;
use flowcontrol::merge;
use flowcontrol::names::Names;
use flowcontrol::vcs::Vcs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ASKED: &str = "annual customers get 10% off upgrades";

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

fn edit(dir: &Path, file: &str, from: &str, to: &str) {
    let p = dir.join(file);
    let s = std::fs::read_to_string(&p).unwrap();
    assert!(s.contains(from), "{from} not in {file}");
    std::fs::write(&p, s.replacen(from, to, 1)).unwrap();
}

fn open(d: &Path) -> Vcs {
    Vcs::open(d, "flows", Names::default()).unwrap()
}

/// The demo app, saved once.
fn repo(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo");
    for e in walkdir::WalkDir::new(&src) {
        let e = e.unwrap();
        let rel = e.path().strip_prefix(&src).unwrap();
        if rel.starts_with("flows") {
            continue;
        }
        if e.file_type().is_dir() {
            std::fs::create_dir_all(d.join(rel)).unwrap();
        } else {
            std::fs::copy(e.path(), d.join(rel)).unwrap();
        }
    }
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    open(&d).save("First").unwrap();
    d
}

fn the_change(d: &Path) {
    edit(d, "lib/billing.ts", "{ pro: 24 }", "{ pro: 22 }");
    edit(d, "app/api/cancel/route.ts", "  renderHeader();", "  renderHeader();\n  S.refund = 0.9;");
    edit(d, "app/api/cancel/route.ts", "import { renderHeader } from \"@/lib/ui\";", "import { renderHeader } from \"@/lib/ui\";\nimport { S } from \"@/lib/state\";");
}

fn comment_on(d: &Path, base: &str, asked: Option<&str>) -> Comment {
    between(&mut open(d), base, None, |a, b, r| write(a, b, &r, asked.map(str::to_string), None)).unwrap()
}

fn ids(v: &[Item]) -> Vec<&str> {
    v.iter().map(|i| i.id.as_str()).collect()
}

#[test]
fn what_nobody_asked_for_is_flagged_with_one_question() {
    let d = repo("c-flag");
    the_change(&d);
    let c = comment_on(&d, "HEAD", Some(ASKED));
    assert_eq!(ids(&c.not_asked), ["POST /api/cancel"]);
    assert_eq!(ids(&c.changed), ["POST /api/upgrade"]);
    let t = render(&c, false);
    assert!(t.contains("Asked:     annual") && t.contains("Is that what you want?") && t.contains("value: 24 -> 22"), "{t}");
    assert!(t.contains("Kept:      no earlier decision was touched"), "{t}");
    assert_eq!(t.matches("Is that what you want?").count(), 1);
    // Word for word the same every time.
    assert_eq!(t, render(&comment_on(&d, "HEAD", Some(ASKED)), false));
}

#[test]
fn with_nothing_asked_it_only_lists_what_changed() {
    let d = repo("c-plain");
    the_change(&d);
    let c = comment_on(&d, "HEAD", None);
    assert!(c.not_asked.is_empty());
    assert_eq!(c.changed.len(), 2);
    assert!(!render(&c, false).contains("Is that what you want?"));
}

#[test]
fn a_word_most_journeys_share_is_not_a_match() {
    let item = |id: &str, title: &str| Item { id: id.into(), title: title.into(), state: JState::Changed, facts: vec![], steps: vec![], weight: 1, key: String::new(), sentence: None, undo: None, also: vec![], why: None };
    let all: Vec<String> = ["Customer upgrades", "Customer cancels", "Customer signs up", "Customer logs in", "Customer pays", "Customer invites"].map(String::from).to_vec();
    let (asked, not) = split("customers get 10% off upgrades", vec![item("a", "Customer upgrades"), item("b", "Customer cancels")], &all);
    assert_eq!(ids(&asked), ["a"]);
    assert_eq!(ids(&not), ["b"]);
}

#[test]
fn the_rarest_word_says_which_journey_was_meant() {
    let item = |id: &str, title: &str| Item { id: id.into(), title: title.into(), state: JState::Changed, facts: vec![], steps: vec![], weight: 1, key: String::new(), sentence: None, undo: None, also: vec![], why: None };
    let all: Vec<String> = ["Customer upgrades annual plan", "Customer cancels annual plan", "Customer signs up"].map(String::from).to_vec();
    let (asked, not) = split("annual customers get 10% off upgrades", vec![item("a", "Customer upgrades annual plan"), item("b", "Customer cancels annual plan")], &all);
    assert_eq!(ids(&asked), ["a"]);
    assert_eq!(ids(&not), ["b"]);
}

#[test]
fn a_request_naming_no_journey_is_matched_on_the_steps_that_changed() {
    let i = Item { id: "x".into(), title: "Checkout".into(), state: JState::Changed, facts: vec![], steps: vec!["Round discount".into()], weight: 1, key: String::new(), sentence: None, undo: None, also: vec![], why: None };
    let all = ["Checkout".to_string()];
    assert_eq!(split("fix the rounding", vec![i.clone()], &all).0.len(), 1);
    assert_eq!(split("change the colours", vec![i], &all).1.len(), 1);
}

#[test]
fn an_answer_is_one_line_and_the_question_is_not_asked_twice() {
    let d = repo("c-yes");
    the_change(&d);
    let said = decide(&mut open(&d), "POST /api/cancel", "yes", None, "HEAD").unwrap();
    let text = std::fs::read_to_string(d.join(FILE)).unwrap();
    assert_eq!(parse(&text).len(), 1);
    assert_eq!(parse(&text)[0].journey.as_deref(), Some("POST /api/cancel"));
    assert!(text.contains(&said) && text.lines().nth(1).unwrap().starts_with("- "));
    let c = comment_on(&d, "HEAD", Some(ASKED));
    assert!(c.not_asked.is_empty());
    assert_eq!(c.decided, ["POST /api/cancel"]);
    assert!(c.touched.is_empty());
    // Reformatting does not reopen it.
    edit(&d, "app/api/cancel/route.ts", "  S.refund = 0.9;", "    S.refund   =   0.9;");
    assert!(comment_on(&d, "HEAD", Some(ASKED)).not_asked.is_empty());
}

#[test]
fn a_later_change_to_a_decided_journey_says_so() {
    let d = repo("c-later");
    the_change(&d);
    decide(&mut open(&d), "POST /api/cancel", "yes", Some("Annual cancels refund 90%"), "HEAD").unwrap();
    open(&d).save("Annual").unwrap();
    edit(&d, "app/api/cancel/route.ts", "S.refund = 0.9", "S.refund = 0.8");
    let c = comment_on(&d, "HEAD", Some("tidy the header"));
    assert_eq!(c.touched.len(), 1);
    assert!(c.touched[0].starts_with("Annual cancels refund 90%"));
    assert!(render(&c, false).contains("Touched:   Annual cancels"));
}

#[test]
fn a_change_elsewhere_leaves_the_decision_kept() {
    let d = repo("c-kept");
    the_change(&d);
    decide(&mut open(&d), "POST /api/cancel", "yes", None, "HEAD").unwrap();
    open(&d).save("Annual").unwrap();
    edit(&d, "lib/billing.ts", "{ pro: 22 }", "{ pro: 21 }");
    let c = comment_on(&d, "HEAD", None);
    assert!(c.touched.is_empty() && c.decided.is_empty());
}

#[test]
fn no_stays_open_until_someone_changes_the_journey_and_then_is_reported() {
    let d = repo("c-no");
    the_change(&d);
    assert!(decide(&mut open(&d), "POST /api/cancel", "no", None, "HEAD").is_err());
    decide(&mut open(&d), "cancel", "no", Some("Refund the full price"), "HEAD").unwrap();
    let c = comment_on(&d, "HEAD", Some(ASKED));
    assert_eq!(ids(&c.not_asked), ["POST /api/cancel"]);
    assert_eq!(c.touched.len(), 1);
}

#[test]
fn a_decision_written_by_hand_counts_when_it_shares_two_words() {
    let d = repo("c-hand");
    std::fs::write(d.join(FILE), "# decisions.md\n- Resolve price stays at 24 for pro. (2026-11-03)\n- Promo and loyalty credit do not stack. (2026-11-03)\n").unwrap();
    open(&d).save("Decide").unwrap();
    the_change(&d);
    let c = comment_on(&d, "HEAD", None);
    assert_eq!(c.may_touch.len(), 1);
    assert!(c.may_touch[0].starts_with("Resolve price"));
}

#[test]
fn deleting_a_decision_is_reported() {
    let d = repo("c-removed");
    std::fs::write(d.join(FILE), "# decisions.md\n- Promo and loyalty credit do not stack. (2026-11-03)\n").unwrap();
    open(&d).save("Decide").unwrap();
    std::fs::write(d.join(FILE), "# decisions.md\n").unwrap();
    let c = comment_on(&d, "HEAD", None);
    assert_eq!(c.removed.len(), 1);
    assert!(render(&c, false).contains("deletes a decision"));
}

#[test]
fn decisions_are_not_reported_as_a_change() {
    let d = repo("c-quiet");
    decide(&mut open(&d), "POST /api/cancel", "yes", None, "HEAD").unwrap();
    assert!(comment_on(&d, "HEAD", None).is_empty());
}

#[test]
fn an_unknown_or_vague_journey_is_refused() {
    let d = repo("c-vague");
    assert!(decide(&mut open(&d), "no such thing", "yes", None, "HEAD").is_err());
    assert!(decide(&mut open(&d), "api", "yes", None, "HEAD").is_err());
    assert!(decide(&mut open(&d), "POST /api/cancel", "maybe", None, "HEAD").is_err());
    assert!(!d.join(FILE).exists());
}

#[test]
fn two_branches_adding_decisions_merge_cleanly_and_keep_both() {
    let d = repo("c-merge");
    std::fs::write(d.join(FILE), "# decisions.md\n").unwrap();
    open(&d).save("Start").unwrap();
    git(&d, &["checkout", "-q", "-b", "a"]);
    std::fs::write(d.join(FILE), "# decisions.md\n- One. (2026-11-03)\n").unwrap();
    open(&d).save("One").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    git(&d, &["checkout", "-q", "-b", "b"]);
    std::fs::write(d.join(FILE), "# decisions.md\n- Two. (2026-11-03)\n").unwrap();
    open(&d).save("Two").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "a").unwrap();
    assert!(p.collisions.is_empty(), "{:?}", p.collisions);
    merge::apply(&v, &p, "a").unwrap();
    let t = std::fs::read_to_string(d.join(FILE)).unwrap();
    assert_eq!(parse(&t).len(), 2);
}

#[test]
fn the_diff_shows_only_touched_journeys_with_changed_steps_marked() {
    let d = repo("c-map");
    the_change(&d);
    let names = Names::default();
    let page = between(&mut open(&d), "HEAD", None, |a, b, r| {
        let c = write(a, b, &r, Some(ASKED.to_string()), None);
        flowcontrol::map::render(a, b, &r, &c, "demo", "HEAD", "the working folder", &names)
    })
    .unwrap();
    let start = page.find("const D = ").unwrap() + "const D = ".len();
    let end = page[start..].find(";\nconst el").unwrap() + start;
    let data: serde_json::Value = serde_json::from_str(&page[start..end].replace("<\\/", "</")).unwrap();
    let j = data["journeys"].as_array().unwrap();
    let titles: Vec<&str> = j.iter().map(|x| x["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["POST /api/cancel", "POST /api/upgrade"]);
    assert_eq!(j[0]["notAsked"], true);
    let up = j[1]["steps"].as_array().unwrap();
    let changed: Vec<&serde_json::Value> = up.iter().filter(|s| s["kind"] == "changed").collect();
    assert_eq!(changed.len(), 1);
    assert!(changed[0]["before"].as_str().unwrap().contains("24") && changed[0]["after"].as_str().unwrap().contains("22"));
    assert!(up.iter().any(|s| s["kind"] == "same"));
}

/// A stand-in for Claude: says the given sentence, and answers "asked" as told.
fn fake(sentence: &'static str, undo: &'static str, asked: bool, touches: Vec<u64>) -> impl Fn(&str, &Value) -> anyhow::Result<Value> + Sync {
    move |_p: &str, schema: &Value| {
        Ok(if schema["properties"].get("sentence").is_some() { json!({"sentence": sentence, "undo": undo}) } else { json!({"asked": asked, "unasked": "", "touches": touches}) })
    }
}

fn saved_comment(d: &Path, asked: Option<&str>) -> Comment {
    let root = d.to_path_buf();
    between(&mut open(d), "HEAD", None, |a, b, r| write_with(a, b, &r, asked.map(str::to_string), None, &Words::load(b, &root))).unwrap()
}

#[test]
fn saved_sentences_replace_the_facts_and_the_comment_never_changes() {
    let d = repo("c-words");
    the_change(&d);
    let ask = fake("Prices change when a customer upgrades", "keep the old price", true, vec![]);
    let f = fill(&mut open(&d), "HEAD", None, None, 2, "claude", &ask).unwrap();
    assert_eq!((f.written, f.refused.len(), f.failed.len()), (2, 0, 0));
    let first = render(&saved_comment(&d, None), false);
    assert!(first.contains("Prices change when a customer upgrades"), "{first}");
    assert!(!first.contains("value: 24"), "{first}");
    assert!(render(&saved_comment(&d, None), true).contains("value: 24"));
    // A second run asks nothing and writes nothing: a stand-in that fails would show it.
    let before = std::fs::read_to_string(d.join(flowcontrol::words::FILE)).unwrap();
    let boom = |_: &str, _: &Value| -> anyhow::Result<Value> { anyhow::bail!("must not be called") };
    let again = fill(&mut open(&d), "HEAD", None, None, 2, "claude", &boom).unwrap();
    assert_eq!((again.written, again.kept, again.failed.len()), (0, 2, 0));
    assert_eq!(before, std::fs::read_to_string(d.join(flowcontrol::words::FILE)).unwrap());
    assert_eq!(first, render(&saved_comment(&d, None), false));
}

#[test]
fn a_sentence_only_describes_the_change_it_was_written_for() {
    let d = repo("c-stale");
    the_change(&d);
    fill(&mut open(&d), "HEAD", None, None, 2, "claude", &fake("Prices change on upgrade", "", true, vec![])).unwrap();
    edit(&d, "lib/billing.ts", "{ pro: 22 }", "{ pro: 21 }");
    // Cancel did not change again, so its sentence stands. Upgrade's no longer applies.
    assert_eq!(render(&saved_comment(&d, None), false).matches("Prices change on upgrade").count(), 1);
}

#[test]
fn a_number_the_code_does_not_have_is_refused() {
    assert!(grounded("Refunds are now 90% of the price", "S.refund = 0.9"));
    assert!(grounded("Upgrades cost 10% less", "x * 0.9"));
    assert!(grounded("The price goes from 24 to 22", "value: 24 -> 22"));
    assert!(!grounded("Upgrades cost 15% less", "x * 0.9"));
    let d = repo("c-refused");
    the_change(&d);
    let f = fill(&mut open(&d), "HEAD", None, None, 2, "claude", &fake("Upgrades now cost 77 dollars", "", true, vec![])).unwrap();
    assert_eq!((f.written, f.refused.len()), (0, 2));
    assert!(render(&saved_comment(&d, None), false).contains("value: 24 -> 22"));
}

#[test]
fn a_saved_judgement_outranks_word_matching_both_ways() {
    let d = repo("c-judge");
    the_change(&d);
    // Words call cancel a surprise and upgrade asked. The saved judgement says neither was asked.
    fill(&mut open(&d), "HEAD", None, Some(ASKED), 2, "claude", &fake("Something changes", "", false, vec![])).unwrap();
    assert_eq!(saved_comment(&d, Some(ASKED)).not_asked.len(), 2);
    // A different request was never judged, so words decide.
    assert_eq!(ids(&saved_comment(&d, Some("tidy upgrades")).changed), ["POST /api/upgrade"]);
}

#[test]
fn a_hand_written_decision_is_a_maybe_until_it_is_judged() {
    let d = repo("c-maybe");
    std::fs::write(d.join(FILE), "# decisions.md\n- Resolve price stays at 24 for pro. (2026-11-03)\n").unwrap();
    open(&d).save("Decide").unwrap();
    the_change(&d);
    let c = saved_comment(&d, None);
    assert_eq!((c.touched.len(), c.may_touch.len()), (0, 1));
    assert!(render(&c, false).contains("May touch: Resolve price"));
    fill(&mut open(&d), "HEAD", None, None, 2, "claude", &fake("Prices change", "", true, vec![1])).unwrap();
    let c = saved_comment(&d, None);
    assert_eq!((c.touched.len(), c.may_touch.len()), (1, 0));
}

#[test]
fn no_can_use_the_old_behaviour_as_its_answer() {
    let d = repo("c-undo");
    the_change(&d);
    fill(&mut open(&d), "HEAD", None, None, 2, "claude", &fake("Refunds change", "refund the full price", true, vec![])).unwrap();
    let t = render(&saved_comment(&d, Some(ASKED)), false);
    assert!(t.contains("no \"refund the full price\""), "{t}");
    decide(&mut open(&d), "POST /api/cancel", "no", None, "HEAD").unwrap();
    assert!(std::fs::read_to_string(d.join(FILE)).unwrap().contains("refund the full price"));
}

#[test]
fn boxes_and_replies_on_a_pull_request_become_decisions() {
    let d = repo("c-answer");
    the_change(&d);
    fill(&mut open(&d), "HEAD", None, None, 2, "claude", &fake("Refunds change", "refund the full price", true, vec![])).unwrap();
    let m = markdown(&saved_comment(&d, Some(ASKED)));
    assert!(m.contains("- [ ] Yes, keep it <!-- flow-yes: POST /api/cancel -->"), "{m}");
    assert!(answers_in(&m).is_empty(), "an unticked box is not an answer");
    let ticked = m.replace("- [ ] Yes, keep it", "- [x] Yes, keep it");
    assert_eq!(answer(&mut open(&d), &ticked, "HEAD").unwrap().len(), 1);
    // The workflow runs on every edit, so the same answer must not pile up.
    answer(&mut open(&d), &ticked, "HEAD").unwrap();
    assert_eq!(parse(&std::fs::read_to_string(d.join(FILE)).unwrap()).len(), 1);
    assert!(saved_comment(&d, Some(ASKED)).not_asked.is_empty());
    assert_eq!(answers_in("/flow no POST /api/cancel: Refund the full price"), [("POST /api/cancel".to_string(), "no".to_string(), Some("Refund the full price".to_string()))]);
    assert!(answer(&mut open(&d), "/flow yes POST /api/cancel\n/flow no POST /api/cancel: x", "HEAD").is_err());
}

#[test]
fn two_branches_adding_sentences_merge_cleanly() {
    let d = repo("c-merge-words");
    let path = d.join(flowcontrol::words::FILE);
    std::fs::write(&path, "").unwrap();
    open(&d).save("Start").unwrap();
    git(&d, &["checkout", "-q", "-b", "a"]);
    std::fs::write(&path, "{\"key\":\"1\",\"journey\":\"j\",\"of\":\"says\",\"sentence\":\"one\"}\n").unwrap();
    open(&d).save("One").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    git(&d, &["checkout", "-q", "-b", "b"]);
    std::fs::write(&path, "{\"key\":\"2\",\"journey\":\"j\",\"of\":\"says\",\"sentence\":\"two\"}\n").unwrap();
    open(&d).save("Two").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "a").unwrap();
    assert!(p.collisions.is_empty(), "{:?}", p.collisions);
    merge::apply(&v, &p, "a").unwrap();
    let w = Words::parse(&std::fs::read_to_string(&path).unwrap());
    assert!(w.says("1").is_some() && w.says("2").is_some());
}

#[test]
fn the_pull_request_description_is_read_in_github_actions() {
    let d = repo("c-action");
    the_change(&d);
    let event = d.join("event.json");
    std::fs::write(&event, json!({"pull_request": {"body": ASKED}}).to_string()).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_flow")).arg("-C").arg(&d).args(["comment", "--markdown"]).env("GITHUB_EVENT_PATH", &event).output().unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(t.contains("**Asked:** annual") && t.contains("**Not asked:**"), "{t}{}", String::from_utf8_lossy(&o.stderr));
}

fn flow(d: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut c = Command::new(env!("CARGO_BIN_EXE_flow")).arg("-C").arg(d).args(args).env("FLOW_AI", "off").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).to_string()
}

#[test]
fn the_agents_prompt_becomes_what_was_asked_and_a_save_clears_it() {
    let d = repo("c-prompt");
    flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    the_change(&d);
    let t = flow(&d, &["comment"], "");
    assert!(t.contains("Asked:     annual") && t.contains("Not asked: POST /api/cancel"), "{t}");
    flow(&d, &["save", "-m", "Annual", "--no-words"], "");
    assert!(!flow(&d, &["comment", "HEAD~1", "HEAD"], "").contains("Asked:"));
}

#[test]
fn the_end_of_a_turn_shows_the_comment_once() {
    let d = repo("c-done");
    assert_eq!(flow(&d, &["hook", "done"], "{}"), "", "nothing changed");
    the_change(&d);
    let out = flow(&d, &["hook", "done"], "{}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["systemMessage"].as_str().unwrap().contains("POST /api/upgrade"));
    assert_eq!(flow(&d, &["hook", "done"], "{}"), "", "nothing new to say");
}

#[test]
fn a_hook_inside_an_agent_flow_started_does_nothing() {
    let d = repo("c-inner");
    the_change(&d);
    let o = Command::new(env!("CARGO_BIN_EXE_flow")).arg("-C").arg(&d).args(["hook", "done"]).env("FLOW_AI", "off").env("FLOW_INNER", "1").stdin(std::process::Stdio::null()).output().unwrap();
    assert!(o.stdout.is_empty());
}

#[test]
fn a_hook_outside_a_repository_stays_silent() {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("c-norepo");
    std::fs::create_dir_all(&d).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_flow")).arg("-C").arg(&d).args(["hook", "done"]).env("FLOW_AI", "off").stdin(std::process::Stdio::null()).output().unwrap();
    assert!(o.status.success() && o.stdout.is_empty() && o.stderr.is_empty());
}

/// A file with a constant only one journey's function reads.
fn with_constant(name: &str) -> PathBuf {
    let d = repo(name);
    edit(&d, "lib/billing.ts", "export function chargeCard(amount: number) {\n  S.charged = (S.charged ?? 0) + amount;", "export const CARD_FEE = 2;\n\nexport function chargeCard(amount: number) {\n  S.charged = (S.charged ?? 0) + amount + CARD_FEE;");
    open(&d).save("Fee").unwrap();
    d
}

#[test]
fn a_changed_constant_reaches_the_journeys_that_read_it() {
    let d = with_constant("c-const");
    edit(&d, "lib/billing.ts", "CARD_FEE = 2", "CARD_FEE = 3");
    let c = comment_on(&d, "HEAD", None);
    assert_eq!(ids(&c.changed), ["POST /api/upgrade"], "{:?}", c.smaller);
    assert!(c.changed[0].facts.iter().any(|f| f.contains("2 -> 3")), "{:?}", c.changed[0].facts);
    assert!(c.smaller.iter().all(|s| !s.contains("no journey runs it")), "{:?}", c.smaller);
}

#[test]
fn a_decision_is_touched_when_a_constant_its_journey_reads_changes() {
    let d = with_constant("c-const-decide");
    edit(&d, "lib/billing.ts", "CARD_FEE = 2", "CARD_FEE = 3");
    decide(&mut open(&d), "POST /api/upgrade", "yes", Some("The card fee is 3"), "HEAD").unwrap();
    open(&d).save("Fee up").unwrap();
    edit(&d, "lib/billing.ts", "CARD_FEE = 3", "CARD_FEE = 4");
    assert_eq!(comment_on(&d, "HEAD", None).touched.len(), 1);
}

#[test]
fn one_change_reaching_many_journeys_is_one_question_and_one_answer() {
    let d = repo("c-group");
    edit(&d, "lib/auth.ts", "export function loadSession() {", "export function loadSession() {\n  S.visits = 1;");
    let first = comment_on(&d, "HEAD", Some("something else entirely"));
    assert_eq!(first.not_asked.len(), 1, "{:?}", ids(&first.not_asked));
    assert!(!first.not_asked[0].also.is_empty());
    let t = render(&first, false);
    assert_eq!(t.matches("Is that what you want?").count(), 1);
    assert!(t.contains("journeys:"), "{t}");
    let id = first.not_asked[0].id.clone();
    decide(&mut open(&d), &id, "yes", Some("Visits are counted"), "HEAD").unwrap();
    let after = comment_on(&d, "HEAD", Some("something else entirely"));
    assert!(after.not_asked.is_empty(), "{:?}", ids(&after.not_asked));
}

#[test]
fn only_one_wording_is_paid_for_when_many_journeys_change_the_same_way() {
    let d = repo("c-one-call");
    edit(&d, "lib/auth.ts", "export function loadSession() {", "export function loadSession() {\n  S.visits = 1;");
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let ask = |_: &str, s: &Value| { calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst); fake("Visits are counted", "", true, vec![])("", s) };
    fill(&mut open(&d), "HEAD", None, None, 2, "claude", &ask).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(render(&saved_comment(&d, None), false).contains("Visits are counted"));
}

#[test]
fn the_part_nobody_asked_for_replaces_the_sentence() {
    let d = repo("c-part");
    the_change(&d);
    let ask = |_: &str, s: &Value| -> anyhow::Result<Value> { Ok(if s["properties"].get("sentence").is_some() { json!({"sentence": "Prices move", "undo": ""}) } else { json!({"asked": false, "unasked": "It also lowers the pro price to 22", "touches": []}) }) };
    fill(&mut open(&d), "HEAD", None, Some(ASKED), 2, "claude", &ask).unwrap();
    assert!(render(&saved_comment(&d, Some(ASKED)), false).contains("It also lowers the pro price to 22"));
}

/// Upgrade and cancel both read one value, so a change to it gives both identical facts.
fn shared_rate(name: &str) -> PathBuf {
    let d = repo(name);
    edit(&d, "lib/billing.ts", "export function resolvePrice", "export const RATE = 1;\nexport function refundFor(paid: number) {\n  return paid * RATE;\n}\n\nexport function resolvePrice");
    edit(&d, "lib/billing.ts", "return table[plan];", "return table[plan] * RATE;");
    edit(&d, "app/api/cancel/route.ts", "import { renderHeader } from \"@/lib/ui\";", "import { renderHeader } from \"@/lib/ui\";\nimport { refundFor } from \"@/lib/billing\";");
    edit(&d, "app/api/cancel/route.ts", "  renderHeader();", "  renderHeader();\n  refundFor(20);");
    open(&d).save("Rate").unwrap();
    edit(&d, "lib/billing.ts", "RATE = 1;", "RATE = 0.9;");
    d
}

#[test]
fn the_same_facts_can_be_asked_for_in_one_journey_and_a_surprise_in_another() {
    let d = shared_rate("c-split");
    let prompts = std::sync::Mutex::new(vec![]);
    let ask = |p: &str, s: &Value| -> anyhow::Result<Value> {
        prompts.lock().unwrap().push(p.to_string());
        if s["properties"].get("sentence").is_none() {
            let cancel = p.lines().find(|l| l.ends_with("POST /api/cancel") && l.starts_with(|c: char| c.is_ascii_digit())).unwrap().split('.').next().unwrap().parse::<u64>().unwrap();
            return Ok(json!({"asked": false, "unasked": "", "not_called_for": [cancel], "touches": []}));
        }
        let also = p.contains("also reaches");
        let cancel = p.contains("Journey: POST /api/cancel") && !also;
        Ok(json!({"sentence": if also { "Annual customers pay less" } else if cancel { "Cancel refunds 90% of what was paid" } else { "Upgrade charges 10% less" }, "undo": "keep the full rate"}))
    };
    let f = fill(&mut open(&d), "HEAD", None, Some(ASKED), 1, "claude", &ask).unwrap();
    assert!(f.failed.is_empty() && f.refused.is_empty(), "{:?} {:?}", f.failed, f.refused);
    let c = saved_comment(&d, Some(ASKED));
    let t = render(&c, false);
    let (no, yes) = t.split_once("Changed:").unwrap_or_else(|| panic!("{t}"));
    assert!(no.contains("Cancel refunds 90%") && !no.contains("Upgrade"), "{t}");
    assert!(yes.contains("Upgrade charges 10% less"), "{t}");
    assert!(prompts.lock().unwrap().iter().any(|p| p.contains("refundFor")), "the writer sees where the value is used");
}

#[test]
fn a_question_after_the_change_does_not_repeat_the_comment_or_alter_what_was_asked() {
    let d = shared_rate("c-turns");
    flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    assert!(flow(&d, &["hook", "done"], "{}").contains("Flow:"));
    flow(&d, &["hook", "prompt"], &json!({"prompt": "what does refundFor do?"}).to_string());
    assert_eq!(flow(&d, &["hook", "done"], "{}"), "", "a turn that changed nothing says nothing");
    assert_eq!(flowcontrol::agent::asked_so_far(&open(&d)).as_deref(), Some(ASKED));
    // Prompts are only held back while the code stands still.
    flow(&d, &["hook", "prompt"], &json!({"prompt": "and lower it to 0.8"}).to_string());
    edit(&d, "lib/billing.ts", "RATE = 0.9;", "RATE = 0.8;");
    assert!(flow(&d, &["hook", "done"], "{}").contains("Flow:"));
    let asked = flowcontrol::agent::asked_so_far(&open(&d)).unwrap();
    assert!(asked.contains("what does refundFor do?") && asked.contains("lower it to 0.8"), "{asked}");
    open(&d).save("Done").unwrap();
    assert_eq!(flowcontrol::agent::asked_so_far(&open(&d)), None);
}

#[test]
fn planning_turns_before_any_change_still_count_as_what_was_asked() {
    let d = shared_rate("c-plan");
    git(&d, &["checkout", "-q", "--", "."]);
    flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    assert_eq!(flow(&d, &["hook", "done"], "{}"), "");
    flow(&d, &["hook", "prompt"], &json!({"prompt": "go ahead"}).to_string());
    edit(&d, "lib/billing.ts", "RATE = 1;", "RATE = 0.9;");
    flow(&d, &["hook", "done"], "{}");
    assert!(flowcontrol::agent::asked_so_far(&open(&d)).unwrap().contains(ASKED));
}

#[test]
fn an_answer_stays_with_the_journeys_shown_with_it() {
    let d = shared_rate("c-sides");
    let ask = |_: &str, s: &Value| -> anyhow::Result<Value> {
        Ok(if s["properties"].get("sentence").is_some() { json!({"sentence": "Annual customers pay less", "undo": ""}) } else { json!({"asked": false, "unasked": "", "not_called_for": [1], "touches": []}) })
    };
    fill(&mut open(&d), "HEAD", None, Some(ASKED), 1, "claude", &ask).unwrap();
    flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    decide(&mut open(&d), "POST /api/cancel", "yes", None, "HEAD").unwrap();
    let text = std::fs::read_to_string(d.join(FILE)).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 1, "{text}");
    assert!(text.contains("POST /api/cancel |") && !text.contains("POST /api/upgrade |"), "{text}");
}

#[test]
fn claude_is_handed_the_decisions_with_each_prompt() {
    let d = repo("c-told");
    let none = flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    assert!(!none.contains("decisions.md"), "no decisions, none to hand over: {none}");
    std::fs::write(d.join("decisions.md"), "# decisions.md\n- Refunds stay full price. (2026-11-04) <!-- flow: POST /api/cancel | open -->\n- Promo and loyalty credit do not stack. (2026-11-03)\n").unwrap();
    let out = flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    let told = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(told.contains("- Refunds stay full price. (2026-11-04) [POST /api/cancel]"), "{told}");
    assert!(told.contains("- Promo and loyalty credit do not stack."), "{told}");
    assert!(!told.contains("<!--"), "{told}");
    the_change(&d);
    assert!(flow(&d, &["comment"], "").contains("Asked:     annual"), "the prompt is still what was asked");
}

#[test]
fn a_long_decisions_file_sends_its_newest_lines_and_counts_the_rest() {
    let d = repo("c-told-long");
    let lines: String = (0..300).map(|i| format!("- Decision number {i} about something that was settled a while ago. (2026-11-04)\n")).collect();
    std::fs::write(d.join("decisions.md"), lines).unwrap();
    let told = flowcontrol::agent::decisions_context(&d).unwrap();
    assert!(told.contains("Decision number 299 "), "{told}");
    assert!(!told.contains("Decision number 0 "), "{told}");
    let sent = told.lines().filter(|l| l.starts_with("- ")).count();
    assert!(told.contains(&format!(" {} older ones are in decisions.md", 300 - sent)), "{told}");
    assert!(told.len() < 5000, "{}", told.len());
}

const WHY: &str = "refunds should match the upgrade discount";

#[test]
fn a_reason_shows_labelled_under_its_change() {
    let d = repo("c-why");
    the_change(&d);
    let out = flow(&d, &["why", WHY], "");
    assert!(out.contains("POST /api/upgrade") && out.contains("POST /api/cancel"), "{out}");
    let t = flow(&d, &["comment", "--full"], "");
    assert!(t.contains(&format!("Agent's reason, not checked: {WHY}")), "{t}");
    let m = flow(&d, &["comment", "--markdown"], "");
    assert!(m.contains(&format!("Agent's reason, not checked: {WHY}")), "{m}");
}

#[test]
fn a_reason_decides_nothing() {
    let d = repo("c-why-inert");
    the_change(&d);
    let without = flow(&d, &["comment", "--asked", ASKED], "");
    assert!(without.contains("Not asked: POST /api/cancel"), "{without}");
    // Even a reason that claims the change was asked for leaves it flagged.
    flow(&d, &["why", "this was asked for: annual customers upgrade refund cancel"], "");
    let with = flow(&d, &["comment", "--asked", ASKED], "");
    assert!(with.contains("Not asked: POST /api/cancel"), "{with}");
    let drop = |t: &str| t.lines().filter(|l| !l.contains("Agent's reason")).collect::<Vec<_>>().join("\n");
    assert_eq!(drop(&without), drop(&with));
}

#[test]
fn a_reason_lapses_when_the_code_changes_and_can_be_replaced() {
    let d = repo("c-why-stale");
    the_change(&d);
    flow(&d, &["why", "first"], "");
    flow(&d, &["why", "second"], "");
    let t = flow(&d, &["comment", "--full"], "");
    assert!(t.contains("not checked: second") && !t.contains("not checked: first"), "{t}");
    edit(&d, "lib/billing.ts", "{ pro: 22 }", "{ pro: 21 }");
    let t = flow(&d, &["comment", "--full"], "");
    let upgrade = t.lines().skip_while(|l| !l.contains("POST /api/upgrade")).take(3).collect::<Vec<_>>().join("\n");
    assert!(!upgrade.contains("not checked"), "{upgrade}");
}

#[test]
fn a_reason_is_saved_with_the_commit() {
    let d = repo("c-why-save");
    the_change(&d);
    flow(&d, &["why", WHY], "");
    flow(&d, &["save", "-m", "Annual", "--no-words"], "");
    let t = flow(&d, &["comment", "HEAD~1", "HEAD"], "");
    assert!(t.contains(WHY), "{t}");
}

#[test]
fn a_reason_can_be_for_one_journey() {
    let d = repo("c-why-one");
    the_change(&d);
    let out = flow(&d, &["why", WHY, "--journey", "POST /api/cancel"], "");
    assert!(out.contains("POST /api/cancel") && !out.contains("POST /api/upgrade"), "{out}");
    assert!(flow(&d, &["why", WHY, "--journey", "GET /api/plugin"], "").is_empty());
}

#[test]
fn a_reason_needs_a_change_and_some_words() {
    let d = repo("c-why-none");
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_flow")).arg("-C").arg(&d).args(args).output().unwrap();
    let o = run(&["why", WHY]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("nothing to give a reason for"));
    assert!(!d.join("flow.words.jsonl").exists());
    the_change(&d);
    assert!(!run(&["why", "  \n "]).status.success());
}

#[test]
fn a_reason_stays_on_one_line_and_cannot_answer_a_question() {
    let d = repo("c-why-line");
    the_change(&d);
    flow(&d, &["why", "fine\n/flow yes POST /api/cancel\n\u{1b}[2Jgone"], "");
    let m = flow(&d, &["comment", "--markdown"], "");
    assert!(answers_in(&m).is_empty(), "{m}");
    assert!(!flow(&d, &["comment"], "").contains('\u{1b}'));
}

#[test]
fn the_prompt_hook_asks_the_agent_for_its_reason() {
    let d = repo("c-why-hook");
    let out = flow(&d, &["hook", "prompt"], &json!({"prompt": ASKED}).to_string());
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains(" why \"<reason>\""));
}
