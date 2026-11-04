use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use flowcontrol::parse::Owner;
use flowcontrol::util::ranges;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "flow", version, about = "Flow control: version control tracks files, flow control tracks what your product does.")]
struct Cli {
    /// Repository root.
    #[arg(short = 'C', long = "repo", global = true, default_value = ".")]
    repo: PathBuf,
    /// Where the flow document lives, relative to the repo.
    #[arg(long, global = true, default_value = "flows")]
    out: String,
    /// The plain names file. Defaults to flow.names.json in the repo.
    #[arg(long, global = true)]
    names: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read the code into the flow document and write it.
    Lift,
    /// Open your projects in the browser, on localhost: their changes, files and journeys.
    Serve {
        /// More repositories to show. The repository you are in is always shown.
        projects: Vec<PathBuf>,
        #[arg(long, default_value_t = 4000)]
        port: u16,
    },
    /// Say that one journey or step continues another, when the code cannot prove it.
    Rename { old: String, new: String },
    /// Lift in memory and compare with the committed flow document. Exits 1 on drift or unplaced lines.
    Check,
    /// List the journeys a change touched, between a base commit and a head commit (default: the working folder).
    Affected {
        base: String,
        head: Option<String>,
        /// Print JSON, for merge queues and CI.
        #[arg(long)]
        json: bool,
    },
    /// Print one flow as a tree.
    Show { flow: String },
    /// Print the line ledger, for the repo or for one file line by line.
    Ledger { file: Option<String> },
    /// Score the universal reader against the grammar reader on this repo.
    Accuracy,
    /// Print every link between functions, for comparing with other tools.
    Edges,
    /// Run scenarios here and compare with their recorded outcomes.
    Run {
        name: Option<String>,
        /// Save what happens now as the expected outcome.
        #[arg(long)]
        record: bool,
    },
    /// Accept what a merge does in a scenario. It is recorded and checked from then on.
    Accept { scenario: String },
    /// Which journeys your unsaved edits change.
    Status {
        /// List every function, not just the summary tables.
        #[arg(long)]
        full: bool,
    },
    /// Save the working folder as a snapshot. The message records which journeys changed.
    Save {
        #[arg(short, long)]
        message: String,
        /// Do not have the agent on this machine word the change first.
        #[arg(long)]
        no_words: bool,
    },
    /// The history of changes, told in journeys.
    Log {
        #[arg(short = 'n', long, default_value_t = 20)]
        count: usize,
    },
    /// Every change that touched one journey.
    History {
        journey: String,
        #[arg(short = 'n', long, default_value_t = 200)]
        count: usize,
    },
    /// What one saved change did, in journeys.
    Explain {
        change: String,
        /// List every function, not just the summary tables.
        #[arg(long)]
        full: bool,
    },

    /// One short comment on a change: what behaves differently, what nobody asked for, what was decided.
    Comment {
        /// Compare from here.
        #[arg(default_value = "HEAD")]
        base: String,
        /// Compare to here. Defaults to the working folder.
        head: Option<String>,
        /// What was asked for: the prompt or the pull request description.
        #[arg(long)]
        asked: Option<String>,
        /// Read what was asked for from a file.
        #[arg(long)]
        asked_file: Option<PathBuf>,
        /// Print Markdown for a pull request.
        #[arg(long)]
        markdown: bool,
        /// Show what is folded away.
        #[arg(long)]
        full: bool,
    },
    /// Answer a comment's question. The answer is one line in decisions.md.
    Decide {
        journey: String,
        /// yes or no.
        answer: String,
        /// What it should do. Needed for no.
        sentence: Option<String>,
        /// The base the comment compared from.
        #[arg(long, default_value = "HEAD")]
        base: String,
    },
    /// Have Claude put a change into plain sentences. They are saved for review; the comment reads them.
    Word {
        #[arg(default_value = "HEAD")]
        base: String,
        head: Option<String>,
        #[arg(long)]
        asked: Option<String>,
        #[arg(long)]
        asked_file: Option<PathBuf>,
        /// claude or codex. Defaults to whichever is installed.
        #[arg(long)]
        with: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// Calls to run at once.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
    },
    /// Connect this repo to the coding agents on this machine, or run as their hook.
    Hook {
        /// install, or what the agent calls: prompt, done, codex.
        what: String,
        arg: Option<String>,
    },
    /// Record the answers in a pull request comment: ticked boxes and /flow replies.
    Answer {
        /// A file holding the comment.
        file: PathBuf,
        #[arg(long, default_value = "HEAD")]
        base: String,
    },
    /// Open the new diff: each journey a change touches, step by step, in the browser.
    Diff {
        #[arg(default_value = "HEAD")]
        base: String,
        head: Option<String>,
        #[arg(long)]
        asked: Option<String>,
        #[arg(long)]
        asked_file: Option<PathBuf>,
        /// Write the page here instead of a temp file.
        #[arg(long)]
        to: Option<PathBuf>,
        /// Write the page without opening it.
        #[arg(long)]
        no_open: bool,
    },

    /// Merge one or more branches function by function, and show where the two sides meet.
    Merge {
        branches: Vec<String>,
        /// Show what would happen without touching any file.
        #[arg(long)]
        dry_run: bool,
        /// Put everything back as it was before the merge.
        #[arg(long)]
        abort: bool,
        /// Do not run scenarios.
        #[arg(long)]
        no_run: bool,
    },
    /// Decide a merge collision: keep ours or take theirs.
    Resolve { step: String, side: String },
    /// Ask Claude for plain names for every journey and step, saved to flow.names.json for review.
    Name {
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        /// Calls to run at once.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Only say how much would be named.
        #[arg(long)]
        dry_run: bool,
        /// Rename everything Claude named before, not just missing or stale names.
        #[arg(long)]
        all: bool,
    },
    /// Ask Claude to group requests into what people come to do, saved to flow.journeys.json for review.
    Journeys {
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        /// Only say how many requests have no journey yet.
        #[arg(long)]
        dry_run: bool,
    },
    /// Ask Claude for one short sentence per change, saved to flow.titles.json for review.
    Titles {
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        /// How many of the newest changes to title.
        #[arg(long, default_value_t = 100)]
        last: usize,
        /// Calls to run at once.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
    },
    /// Open every journey in the browser: steps, code, and what each step reads and changes.
    View {
        /// Write the page here instead of a temp file.
        #[arg(long)]
        to: Option<PathBuf>,
        /// Write the page without opening it.
        #[arg(long)]
        no_open: bool,
    },
}

fn main() -> ExitCode {
    // Stop quietly when the reader goes away (`flow log | head`), like git does.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("flow: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// What was asked: said here, or from the pull request, or the prompts the agent was given.
fn asked_of(v: &flowcontrol::vcs::Vcs, asked: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    Ok(flowcontrol::comment::asked_text(asked, file)?.or_else(|| flowcontrol::agent::asked_so_far(v)))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn summary(l: &flowcontrol::Lift) {
    let s = &l.out.stats;
    println!(
        "Lifted {} and {} into {} and {}.",
        plural(s.files, "file", "files"),
        plural(s.lines, "line", "lines"),
        plural(s.slices, "slice", "slices"),
        plural(s.flows, "flow", "flows")
    );
    if s.unplaced.is_empty() && s.broken_round_trip.is_empty() {
        println!("Every line has exactly one owner{}.", if s.placed_by_hand > 0 { format!(" ({} placed by hand)", plural(s.placed_by_hand, "line", "lines")) } else { String::new() });
    } else {
        println!("\n{} to nothing. Place {} with an [[assign]] in flow.toml:", plural(s.unplaced.len(), "statement belongs", "statements belong"), if s.unplaced.len() == 1 { "it" } else { "them" });
        for (f, line, code, why) in &s.unplaced {
            println!("  {f}:{line}  {code}\n      {why}");
        }
    }
    for f in &s.broken_round_trip {
        println!("  {f}: the slices do not print back to the file. This is a bug in the lifter.");
    }
    for f in &s.parse_errors {
        println!("  {f}  has syntax the parser could not read, so calls near it may be missing from flows.");
    }
    if !s.unresolved.is_empty() {
        println!("\n{} could not be followed:", plural(s.unresolved.len(), "call", "calls"));
        for (f, line, code, why) in s.unresolved.iter().take(40) {
            println!("  {f}:{line}  {code}\n      {why}");
        }
        if s.unresolved.len() > 40 {
            println!("  … and {} more in flows/_index.yaml and the flow files.", s.unresolved.len() - 40);
        }
    }
    if !s.loose.is_empty() {
        println!("\n{} that no flow reaches:", plural(s.loose.len(), "function", "functions"));
        for x in s.loose.iter().take(25) {
            println!("  {x}");
        }
        if s.loose.len() > 25 {
            println!("  … and {} more in flows/_slices.yaml.", s.loose.len() - 25);
        }
    }
    println!("\nFingerprint {}", l.out.fingerprint);
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let out_dir = cli.repo.join(&cli.out);
    let names_path = cli.names.clone().unwrap_or_else(|| cli.repo.join("flow.names.json"));
    match cli.cmd {
        Cmd::Lift => {
            // A rename can only be seen against the last saved snapshot, so outside git there are none.
            let l = match flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, Default::default()) {
                Ok(mut v) => {
                    let (l, found) = v.now()?;
                    v.record_ids(&l)?;
                    for (a, b) in &found.renamed {
                        println!("Renamed: {a} is now {b}. Its history continues.");
                    }
                    if !found.gone.is_empty() {
                        println!("Gone: {}\nNew: {}\nNothing in the code ties these together. If one continues another, run `flow rename <old> <new>`.", found.gone.join(", "), found.new.join(", "));
                    }
                    l
                }
                Err(_) => flowcontrol::lift(&cli.repo, &cli.out)?,
            };
            summary(&l);
            let n = flowcontrol::write(&out_dir, &l.out.files)?;
            println!("Wrote {} ({} changed).", out_dir.display(), plural(n, "file", "files"));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Serve { projects, port } => {
            let mut roots = vec![cli.repo.clone()];
            roots.extend(projects);
            flowcontrol::serve::run(&roots, &cli.out, port)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Rename { old, new } => {
            let v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, Default::default())?;
            let mut l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let mut ids = l.ids.clone();
            if l.flows.iter().any(|f| f.id == old) || l.uids.steps.contains_key(&old) {
                bail!("{old:?} is still in the code, so {new:?} cannot replace it");
            }
            if !ids.link(&l, &old, &new) {
                bail!("{new:?} is not a journey or step in the code");
            }
            flowcontrol::stamp(&mut l, ids);
            v.record_ids(&l)?;
            if out_dir.is_dir() {
                flowcontrol::write(&out_dir, &l.out.files)?;
            }
            println!("{new} now continues the history of {old}.");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Check => {
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let disk = flowcontrol::read_existing(&out_dir)?;
            let d = flowcontrol::drift(&l.out.files, &disk);
            let s = &l.out.stats;
            let mut ok = true;
            if !d.is_clean() {
                ok = false;
                println!("Drift: the code no longer matches the flow document in {}.", out_dir.display());
                for k in &d.changed {
                    println!("  changed  {k}");
                }
                for k in &d.added {
                    println!("  new      {k}");
                }
                for k in &d.removed {
                    println!("  gone     {k}");
                }
                println!("Run `flow lift` and review the flow changes, or undo the code edits.");
            }
            if !s.unplaced.is_empty() || !s.broken_round_trip.is_empty() {
                ok = false;
                println!("{} to nothing:", plural(s.unplaced.len(), "statement still belongs", "statements still belong"));
                for (f, line, code, _) in &s.unplaced {
                    println!("  {f}:{line}  {code}");
                }
            }
            if ok {
                println!("In sync. {} flows, every line owned, fingerprint {}.", s.flows, l.out.fingerprint);
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        Cmd::Affected { base, head, json } => {
            use flowcontrol::affected::How;
            use flowcontrol::source::Source;
            let rev = |r: &str| Source::Rev { root: cli.repo.clone(), rev: r.to_string() };
            let a = flowcontrol::lift_source(&rev(&base), &cli.out)?;
            let b = match &head {
                Some(h) => flowcontrol::lift_source(&rev(h), &cli.out)?,
                None => flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, Default::default()).and_then(|mut v| v.now()).map(|x| x.0).or_else(|_| flowcontrol::lift(&cli.repo, &cli.out))?,
            };
            let rep = flowcontrol::affected::affected(&a, &b);
            if json {
                println!("{}", serde_json::to_string_pretty(&rep)?);
                return Ok(ExitCode::SUCCESS);
            }
            let to = head.as_deref().unwrap_or("the working folder");
            if rep.is_empty() {
                println!("No journey changed between {base} and {to} ({} checked).", plural(rep.unchanged, "flow", "flows"));
                return Ok(ExitCode::SUCCESS);
            }
            println!("{} changed between {base} and {to}, {} did not.", plural(rep.flows.len(), "journey", "journeys"), rep.unchanged);
            for f in &rep.flows {
                let tag = match f.how { How::Added => "new    ", How::Removed => "gone   ", How::Changed => "changed" };
                println!("\n  {tag}  {}", f.id);
                if f.how == How::Changed {
                    for s in &f.edited { println!("             edited   {s}"); }
                    for s in &f.entered { println!("             now runs {s}"); }
                    for s in &f.left { println!("             no longer runs {s}"); }
                    if f.reordered { println!("             runs its steps in a different order"); }
                }
                if !f.writes.is_empty() {
                    println!("             changes  {}", f.writes.iter().map(|k| k.rsplit('#').next().unwrap_or(k)).collect::<Vec<_>>().join(", "));
                }
            }
            if !rep.loose.is_empty() {
                println!("\nChanged code that no flow runs:");
                for s in &rep.loose { println!("  {s}"); }
            }
            if !rep.unplaced.is_empty() {
                println!("\nLoad-time code with no owner changed, so this list may be incomplete:");
                for s in &rep.unplaced { println!("  {s}"); }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Show { flow } => {
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let Some(f) = l.flows.iter().find(|f| f.id == flow || f.slug == flow) else {
                let ids: Vec<&str> = l.flows.iter().map(|f| f.id.as_str()).collect();
                bail!("no flow {flow:?}. Flows: {}", ids.join(", "));
            };
            println!("{}  ({})", f.id, f.kind);
            for (i, s) in f.steps.iter().enumerate() {
                let sl = l.repo.slice(s.at);
                let via = if s.via == "call" || s.via == "entry" { String::new() } else { format!("  [{}]", s.via) };
                println!(
                    "{:>3}  {}{}{}   {}:{}",
                    i + 1,
                    "  ".repeat(s.depth),
                    sl.name,
                    via,
                    l.repo.files[s.at.0].path,
                    ranges(&sl.lines)
                );
                let pad = " ".repeat(7 + s.depth * 2);
                if let Some(r) = l.graph.reads.get(&s.at) {
                    println!("{pad}reads {}", r.iter().map(|k| k.rsplit('#').next().unwrap_or(k)).collect::<Vec<_>>().join(", "));
                }
                if let Some(w) = l.graph.writes.get(&s.at) {
                    println!("{pad}changes {}", w.iter().map(|k| k.rsplit('#').next().unwrap_or(k)).collect::<Vec<_>>().join(", "));
                }
            }
            if !f.outside.is_empty() {
                println!("\nReaches outside the repo: {}", f.outside.iter().cloned().collect::<Vec<_>>().join(", "));
            }
            if !f.unresolved.is_empty() {
                println!("\nNot followed, so this flow may do more than shown:");
                for (at, u) in &f.unresolved {
                    println!("  {}:{}  {}\n      {}", l.repo.files[at.0].path, u.line + 1, u.code, u.why);
                }
            }
            if f.not_followed > 0 {
                println!("\nMethod calls on values that were not followed: {}.", f.not_followed);
            }
            println!("\nUnverified: no scenario runs this flow yet.");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Edges => {
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let mut v: Vec<String> = l.graph.edges.iter().flat_map(|(a, es)| es.iter().map(|e| format!("{}\t{}", l.repo.sid(*a), l.repo.sid(e.to))).collect::<Vec<_>>()).collect();
            v.sort();
            println!("{}", v.join("\n"));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Accuracy => {
            let s = flowcontrol::accuracy::measure(&cli.repo, &cli.out)?;
            println!("{} journeys compared, {} with no starting point found.", s.journeys, s.missing_entries);
            println!("Precision {:.1}%  recall {:.1}%  journeys exactly right {}.", s.precision() * 100.0, s.recall() * 100.0, s.exact);
            let top = |m: &std::collections::BTreeMap<String, usize>| {
                let mut v: Vec<_> = m.iter().collect();
                v.sort_by(|a, b| b.1.cmp(a.1));
                v.into_iter().take(10).map(|(k, n)| format!("    {n:>4}  {k}")).collect::<Vec<_>>().join("\n")
            };
            println!("Linked wrongly most often:\n{}", top(&s.extras));
            println!("Missed most often:\n{}", top(&s.misses));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Status { full } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let r = v.status()?;
            print!("{}", if full { flowcontrol::change::render(&r) } else { flowcontrol::change::compact(&r) });
            if !full && !r.is_empty() {
                println!("\nEvery function: flow status --full");
            }
            if v.merge_state_path().exists() {
                let st: flowcontrol::merge::MergeState = serde_json::from_str(&std::fs::read_to_string(v.merge_state_path())?)?;
                let open: Vec<_> = st.collisions.iter().filter(|c| !c.resolved).collect();
                println!("\nMerging {}. {} collision(s) left to decide.", st.theirs_name, open.len());
                for c in open {
                    println!("  {}  ({})", c.label, c.file);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Save { message, no_words } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            if !no_words && v.head().is_some() {
                let asked = flowcontrol::agent::asked_so_far(&v);
                let agent = flowcontrol::agent::pick(&v.root, None);
                flowcontrol::agent::describe(&mut v, agent, asked.as_deref());
            }
            if let Some((checks, theirs)) = flowcontrol::merge::refresh(&v)? {
                println!("The code changed since the merge ran, so it ran again:");
                for c in &checks {
                    print!("{}", flowcontrol::scenario::render(c, &theirs));
                }
                println!();
            }
            let (id, r) = v.save(&message)?;
            println!("Saved {}: {}", &id[..id.len().min(8)], flowcontrol::change::one_line(&r));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Log { count } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            for c in v.commits_lifted(count, "HEAD")? {
                let r = v.change_of(&c)?;
                println!("{}  {}  {}{}", &c.id[..8], c.date, c.subject, if c.parents.len() > 1 { "  (merge)" } else { "" });
                println!("          {}", flowcontrol::change::one_line(&r));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::History { journey, count } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let q = journey.to_lowercase();
            let mut title = None;
            let mut hits = 0;
            // Follow the journey itself, under whatever name it had at the time.
            let uid = v.head().and_then(|h| v.lift_rev(&h).ok().and_then(|l| l.journey(&journey).map(|fi| l.flows[fi].uid.clone())));
            for c in v.commits_lifted(count, "HEAD")? {
                let r = v.change_of(&c)?;
                for j in r.journeys.iter().filter(|j| match &uid {
                    Some(u) => &j.uid == u,
                    None => j.id.to_lowercase() == q || j.title.to_lowercase() == q || j.title.to_lowercase().contains(&q),
                }) {
                    if title.is_none() {
                        title = Some(j.title.clone());
                        println!("{}\n", j.title);
                    }
                    hits += 1;
                    let what = match j.state {
                        flowcontrol::change::JState::New => "journey first appears".to_string(),
                        flowcontrol::change::JState::Gone => "journey removed".to_string(),
                        flowcontrol::change::JState::Changed => {
                            let mut parts = vec![];
                            if let Some(w) = &j.was { parts.push(format!("renamed from {w}")); }
                            if !j.edited.is_empty() { parts.push(format!("changed {}", j.edited.join(", "))); }
                            if !j.added.is_empty() { parts.push(format!("added {}", j.added.join(", "))); }
                            if !j.removed.is_empty() { parts.push(format!("dropped {}", j.removed.join(", "))); }
                            if !j.now_changes.is_empty() { parts.push(format!("now changes {}", j.now_changes.join(", "))); }
                            parts.join("; ")
                        }
                    };
                    println!("{}  {}  {}\n          {}", &c.id[..8], c.date, c.subject, what);
                    break;
                }
            }
            if hits == 0 {
                println!("No saved change touched a journey matching {journey:?}.");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Comment { base, head, asked, asked_file, markdown, full } => {
            use flowcontrol::comment as c;
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let asked = asked_of(&v, asked, asked_file)?;
            let shown = (base != "HEAD").then(|| base.clone());
            let root = v.root.clone();
            let out = c::between(&mut v, &base, head.as_deref(), |a, b, r| c::write_with(a, b, &r, asked, shown, &flowcontrol::words::Words::load(b, &root)))?;
            print!("{}", if markdown { c::markdown(&out) } else { c::render(&out, full) });
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Decide { journey, answer, sentence, base } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let said = flowcontrol::comment::decide(&mut v, &journey, &answer, sentence.as_deref(), &base)?;
            println!("Saved to {}: {said}", flowcontrol::comment::FILE);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Word { base, head, asked, asked_file, with, model, jobs } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let asked = asked_of(&v, asked, asked_file)?;
            let Some(agent) = flowcontrol::agent::pick(&v.root, with.as_deref()) else { bail!("no coding agent is available here, so the plain facts stand. Install claude or codex, or check FLOW_AI") };
            let ask = |p: &str, s: &serde_json::Value| flowcontrol::agent::ask(agent, model.as_deref(), p, s);
            let f = flowcontrol::words::fill(&mut v, &base, head.as_deref(), asked.as_deref(), jobs, agent.name(), &ask)?;
            println!("{} written, {} already there. Saved in {}.", plural(f.written, "sentence", "sentences"), f.kept, flowcontrol::words::FILE);
            for r in &f.refused {
                println!("  refused, the code does not support it, so the facts stay: {r}");
            }
            for r in &f.failed {
                println!("  failed: {r}");
            }
            if f.skipped > 0 {
                println!("  {} smaller journeys keep their facts: this change is large.", f.skipped);
            }
            Ok(if f.failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Hook { what, arg } => {
            let names = flowcontrol::names::Names::load(&names_path)?;
            if what == "install" {
                let v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, names)?;
                print!("{}", flowcontrol::agent::install(&v)?);
            } else {
                // Never in the agent's way: a hook that fails stays silent.
                let _ = flowcontrol::agent::hook(&cli.repo, &cli.out, names, &what, arg.as_deref());
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Answer { file, base } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let saved = flowcontrol::comment::answer(&mut v, &std::fs::read_to_string(&file)?, &base)?;
            for s in &saved {
                println!("Saved to {}: {s}", flowcontrol::comment::FILE);
            }
            if saved.is_empty() {
                println!("No answers in {}.", file.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Diff { base, head, asked, asked_file, to, no_open } => {
            use flowcontrol::comment as c;
            let names = flowcontrol::names::Names::load(&names_path)?;
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, names.clone())?;
            let asked = asked_of(&v, asked, asked_file)?;
            let name = v.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
            let end = head.clone().unwrap_or_else(|| "the working folder".into());
            let root = v.root.clone();
            let shown = (base != "HEAD").then(|| base.clone());
            let page = c::between(&mut v, &base, head.as_deref(), |a, b, r| {
                let out = c::write_with(a, b, &r, asked, shown, &flowcontrol::words::Words::load(b, &root));
                flowcontrol::map::render(a, b, &r, &out, &name, &base, &end, &names)
            })?;
            let path = to.unwrap_or_else(|| std::env::temp_dir().join(format!("flow-diff-{}.html", flowcontrol::util::slug(&name))));
            std::fs::write(&path, page)?;
            println!("The change is drawn in {}", path.display());
            if !no_open {
                let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
                let _ = std::process::Command::new(opener).arg(&path).status();
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Explain { change, full } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let id = v.resolve(&change)?;
            let c = v.commits(1, &id)?.remove(0);
            println!("{}  {}  {}\n", &c.id[..8], c.date, c.subject);
            let r = v.change_of(&c)?;
            print!("{}", if full { flowcontrol::change::render(&r) } else { flowcontrol::change::compact(&r) });
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Run { name, record } => {
            let cfg = flowcontrol::config::load(&cli.repo)?;
            let list: Vec<_> = cfg.scenario.iter().filter(|s| name.as_ref().map(|n| n == &s.name).unwrap_or(true)).collect();
            if list.is_empty() {
                bail!("no scenario found. Add one to flow.toml:\n\n[[scenario]]\nname = \"upgrade\"\njourney = \"POST /api/upgrade\"\nrun = \"node scenarios/upgrade.mjs\"");
            }
            let mut bad = 0;
            for s in list {
                let (a, b) = (flowcontrol::scenario::run(&cli.repo, s, "here")?, flowcontrol::scenario::run(&cli.repo, s, "here")?);
                let stable: flowcontrol::scenario::Outcome = a.iter().filter(|(k, v)| b.get(*k) == Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
                let unstable = a.len() - stable.len();
                let note = if unstable > 0 { format!(" ({unstable} value(s) ignored, different on every run)") } else { String::new() };
                if record {
                    flowcontrol::scenario::record(&cli.repo, &cli.out, &s.name, &stable)?;
                    println!("{}: recorded {} value(s){note}.", s.name, stable.len());
                    continue;
                }
                match flowcontrol::scenario::recorded(&cli.repo, &cli.out, &s.name) {
                    None => {
                        println!("{}: ran, nothing recorded to compare with yet{note}.", s.name);
                        for (k, v) in &stable {
                            println!("    {k} = {v}");
                        }
                        if stable.contains_key("failed") {
                            bad += 1;
                        }
                    }
                    Some(want) => {
                        let diff: Vec<String> = want.keys().chain(stable.keys()).collect::<std::collections::BTreeSet<_>>().into_iter().filter(|k| want.get(*k) != stable.get(*k)).map(|k| format!("    {k}: expected {}, got {}", want.get(k).map(String::as_str).unwrap_or("nothing"), stable.get(k).map(String::as_str).unwrap_or("nothing"))).collect();
                        if diff.is_empty() {
                            println!("{}: as recorded{note}.", s.name);
                        } else {
                            bad += 1;
                            println!("{}: different from what was recorded:\n{}", s.name, diff.join("\n"));
                        }
                    }
                }
            }
            Ok(if bad == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Accept { scenario } => {
            let v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            flowcontrol::merge::accept(&v, &scenario)?;
            println!("Accepted. {scenario} is recorded, and any change that breaks it will be flagged.");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Merge { branches, dry_run, abort, no_run } => {
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            if abort {
                flowcontrol::merge::abort(&v)?;
                println!("Merge undone. The folder is back to the last saved snapshot.");
                return Ok(ExitCode::SUCCESS);
            }
            if branches.is_empty() {
                bail!("name the branch to merge");
            }
            let branch = branches.join(", ");
            let p = flowcontrol::merge::plan_many(&mut v, &branches)?;
            if p.already_in {
                println!("Everything in {branch} is already here.");
                return Ok(ExitCode::SUCCESS);
            }
            if p.fast_forward {
                let c = v.commits(1, &p.theirs)?.remove(0);
                let mut total = flowcontrol::change::Report::default();
                let _ = &mut total;
                if !dry_run {
                    flowcontrol::merge::apply(&v, &p, &branch)?;
                }
                println!("Nothing changed here since {branch} split off, so all of it is taken as it is{}.", if dry_run { " (dry run)" } else { "" });
                println!("Latest change on {branch}: {}  {}", &c.id[..8], c.subject);
                return Ok(ExitCode::SUCCESS);
            }
            let taken: Vec<String> = p.taken.iter().map(|(f, n)| if n.is_empty() { f.clone() } else { format!("{} ({f})", flowcontrol::util::human(n)) }).collect();
            println!("From {branch}: {} taken as they are.", plural(taken.len(), "change", "changes"));
            for t in &taken {
                println!("  {t}");
            }
            if !p.moved.is_empty() {
                println!("\nMoved on one side and edited on the other, merged at the new place:");
                for (n, from, to) in &p.moved {
                    println!("  {}  {from} -> {to}", flowcontrol::util::human(n));
                }
            }
            if !p.combined.is_empty() {
                println!("\nChanged on both sides in different lines of the same function, combined. Check these read right:");
                for (f, n) in &p.combined {
                    println!("  {}  ({f})", flowcontrol::util::human(n));
                }
            }
            if !p.text_merged.is_empty() {
                println!("\nFiles that are not code, changed on both sides, merged as text: {}", p.text_merged.join(", "));
            }
            if p.collisions.is_empty() {
                println!("\nNo collisions: no function was changed differently on both sides.");
            } else {
                println!("\n{} changed differently on both sides. Our version is kept until you decide:", plural(p.collisions.len(), "thing was", "things were"));
                for c in &p.collisions {
                    println!("  {}  ({}){}", c.label, c.file, if c.journeys.is_empty() { String::new() } else { format!("\n      in {}", c.journeys.join(", ")) });
                }
            }
            if !p.meets.is_empty() {
                println!("\nThe two sides meet in {}. Each side changed a different step, so the text merges cleanly, but nobody has seen them together:", plural(p.meets.len(), "journey", "journeys"));
                for (j, o, t) in &p.meets {
                    println!("  {j}\n      here: {}\n      {branch}: {}", o.join(", "), t.join(", "));
                }
            }
            if p.blocked.is_some() {
                bail!("{branch} cannot merge all at once, and nothing was written. Merge them one at a time, deciding collisions as you go");
            }
            let mut checks = vec![];
            if !no_run && p.collisions.is_empty() {
                checks = flowcontrol::scenario::check_merge(&v, &p, &p.base)?;
                if !checks.is_empty() {
                    println!("\nRan {} on the versions before, each side, and merged:", plural(checks.len(), "scenario", "scenarios"));
                    for c in &checks {
                        print!("{}", flowcontrol::scenario::render(c, &branch));
                    }
                }
            }
            for (i, (j, ..)) in p.meets.iter().enumerate() {
                let covered = checks.iter().any(|c| c.journey.is_none() || c.journey.as_deref() == Some(j.as_str()) || c.journey.as_deref() == Some(p.meet_ids[i].as_str()));
                if !covered {
                    println!("\nNo scenario runs {j}, so that meet is unchecked.");
                }
            }
            if dry_run {
                println!("\nDry run: nothing was written.");
                return Ok(ExitCode::SUCCESS);
            }
            let blocked: Vec<String> = checks.iter().filter(|c| !c.clean()).map(|c| c.scenario.clone()).collect();
            flowcontrol::merge::apply_checked(&v, &p, &branch, checks)?;
            if !blocked.is_empty() {
                println!("\nWritten to the folder, but saving is blocked until someone decides. Change the code, or accept the result with `flow accept {}`.", blocked[0]);
                return Ok(ExitCode::SUCCESS);
            }
            println!("\nWritten to the folder. {}", if p.collisions.is_empty() { "Run `flow save -m \"...\"` to record the merge." } else { "Decide each collision with `flow resolve <step> ours|theirs`, then `flow save`." });
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Resolve { step, side } => {
            let v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let label = flowcontrol::merge::resolve(&v, &step, &side)?;
            println!("{label}: keeping {side}.");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Journeys { model, dry_run } => {
            use flowcontrol::journeys::{self, Journeys};
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let path = cli.repo.join(journeys::FILE);
            let mut book = Journeys::load(&path)?;
            let todo = journeys::unplaced(&l, &book, &l.ids).len();
            println!("{todo} of {} requests have no journey yet.", l.flows.len());
            if dry_run || todo == 0 {
                return Ok(ExitCode::SUCCESS);
            }
            let rep = journeys::group(&l, &mut book, &l.ids, &model)?;
            book.save(&path)?;
            println!("Placed {} requests in {} journeys. Saved to {}.", rep.placed, book.journeys.len(), path.display());
            println!("This is a proposal: rename a journey or move a request by editing that file. Running this again only places new requests.");
            for r in &rep.left {
                println!("  no journey: {r}");
            }
            Ok(if rep.left.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Titles { model, last, jobs } => {
            use flowcontrol::{journeys, titles};
            let mut v = flowcontrol::vcs::Vcs::open(&cli.repo, &cli.out, flowcontrol::names::Names::load(&names_path)?)?;
            let path = cli.repo.join(titles::FILE);
            let mut have = titles::load(&path)?;
            let book = journeys::Journeys::load(&cli.repo.join(journeys::FILE))?;
            let rep = titles::write(&mut v, &book, &mut have, last, &model, jobs)?;
            titles::save(&have, &path)?;
            println!("Titled {} changes. Saved to {}. Edit any title in that file; running this again only fills in new changes.", rep.titled, path.display());
            for f in &rep.failures {
                println!("  failed: {f}");
            }
            Ok(if rep.failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Name { model, jobs, dry_run, all } => {
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let mut names = flowcontrol::names::Names::load(&names_path)?;
            let opts = flowcontrol::names::Options { model, jobs, dry_run, all };
            let rep = flowcontrol::names::name(&l, &mut names, &opts)?;
            if dry_run {
                return Ok(ExitCode::SUCCESS);
            }
            names.save(&names_path)?;
            println!("Named {} journeys and {} steps in {} calls. Saved to {}.", rep.journeys_named, rep.steps_named, rep.calls, names_path.display());
            println!("These are proposals: edit any name in that file, and set \"by\": \"person\" to keep it as yours.");
            for f in &rep.failures {
                println!("  failed: {f}");
            }
            Ok(if rep.failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::View { to, no_open } => {
            let l = flowcontrol::lift(&cli.repo, &cli.out)?;
            let names = flowcontrol::names::Names::load(&names_path)?;
            let abs = std::fs::canonicalize(&cli.repo)?;
            let name = abs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
            let path = to.unwrap_or_else(|| std::env::temp_dir().join(format!("flow-view-{}.html", flowcontrol::util::slug(&name))));
            let history = flowcontrol::view::history(&cli.repo, &cli.out, &names, 40);
            std::fs::write(&path, flowcontrol::view::render_with_history(&l, &name, &names, history))?;
            println!("{} journeys from {name} written to {}", l.flows.len(), path.display());
            if !no_open {
                let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
                let _ = std::process::Command::new(opener).arg(&path).status();
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Ledger { file } => {
            let repo = flowcontrol::repo::Repo::load(&cli.repo, &cli.out)?;
            match file {
                Some(p) => {
                    let Some(fi) = repo.file_idx(&p) else { bail!("{p} is not a source file in this repo") };
                    let f = &repo.files[fi];
                    let w = f.slices.iter().map(|s| s.name.len()).max().unwrap_or(8).min(28);
                    for (r, line) in f.lines.iter().enumerate() {
                        let who = match f.owner[r] {
                            Owner::Slice(i) => {
                                let s = &f.slices[i];
                                if s.hand.contains(&r) { format!("{} (by hand)", s.name) } else { s.name.clone() }
                            }
                            Owner::Unplaced(_) => "UNPLACED".to_string(),
                        };
                        println!("{:>4} │ {:<w$} │ {}", r + 1, who, line, w = w);
                    }
                    let l = f.ledger();
                    println!("\n{} lines: {} placed + {} unplaced. Round trip {}.", l.total, l.placed, l.unplaced_lines, if l.complete && l.round_trip { "exact" } else { "BROKEN" });
                }
                None => {
                    let (mut t, mut p, mut u) = (0, 0, 0);
                    for f in &repo.files {
                        let l = f.ledger();
                        t += l.total;
                        p += l.placed;
                        u += l.unplaced_lines;
                        if l.unplaced_lines > 0 || !(l.complete && l.round_trip) {
                            println!("{:>5} lines  {:>3} unplaced  {}{}", l.total, l.unplaced_lines, f.path, if l.round_trip { "" } else { "  ROUND TRIP BROKEN" });
                        }
                    }
                    println!("{} files, {t} lines: {p} placed + {u} unplaced.", repo.files.len());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
