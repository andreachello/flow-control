//! Flow can use the coding agent already on this machine, on its owner's own subscription, so it
//! needs no key of its own. Git never does this, so everything here is optional: with no agent,
//! as on a CI runner, flow reports plain facts and matches words.
//!
//! Hooks tell flow when to fire. The agent hands over each prompt, which becomes what was asked,
//! and says when a turn ends, which is when a change is ready to describe.

use crate::comment;
use crate::vcs::Vcs;
use crate::words;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set on agents flow starts, so their own hooks cannot start flow again.
const INNER: &str = "FLOW_INNER";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// The agent to use: asked for by name, or FLOW_AI, or `ai` in flow.toml, or whichever is
/// installed. "off" means none.
pub fn pick(root: &Path, wanted: Option<&str>) -> Option<Agent> {
    let env = std::env::var("FLOW_AI").ok();
    let cfg = crate::config::load(root).ok().and_then(|c| c.ai);
    let chosen = wanted.map(str::to_string).or(env).or(cfg);
    match chosen.as_deref() {
        Some("off") => None,
        Some("claude") => on_path("claude").then_some(Agent::Claude),
        Some("codex") => on_path("codex").then_some(Agent::Codex),
        _ if on_path("claude") => Some(Agent::Claude),
        _ if on_path("codex") => Some(Agent::Codex),
        _ => None,
    }
}

fn run_codex(model: Option<&str>, prompt: &str, schema: &Value) -> Result<Value> {
    // In an empty folder, so it answers from the prompt and cannot wander through the repo.
    let dir = std::env::temp_dir().join(format!("flow-codex-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    std::fs::create_dir_all(&dir)?;
    let (sf, of) = (dir.join("schema.json"), dir.join("out.json"));
    std::fs::write(&sf, schema.to_string())?;
    let mut c = Command::new("codex");
    c.args(["exec", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only", "-c", "notify=[]", "--output-schema"]).arg(&sf).arg("-o").arg(&of).arg("-C").arg(&dir);
    if let Some(m) = model {
        c.args(["-m", m]);
    }
    let mut child = c.arg("-").env(INNER, "1").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("could not run codex")?;
    child.stdin.take().context("no stdin")?.write_all(prompt.as_bytes())?;
    let done = child.wait_with_output()?;
    let text = std::fs::read_to_string(&of).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if !done.status.success() {
        bail!("codex failed: {}", String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or(""));
    }
    serde_json::from_str(&text).context("codex did not return the expected answer")
}

pub fn ask(agent: Agent, model: Option<&str>, prompt: &str, schema: &Value) -> Result<Value> {
    match agent {
        Agent::Claude => crate::names::run_claude(model.unwrap_or("claude-opus-5-5"), prompt, schema),
        Agent::Codex => run_codex(model, prompt, schema),
    }
}

/// Put the current change into sentences, if an agent is here. Never stops the work around it.
pub fn describe(v: &mut Vcs, agent: Option<Agent>, asked: Option<&str>) -> Option<words::Filled> {
    let a = agent?;
    let call = move |p: &str, s: &Value| ask(a, None, p, s);
    words::fill(v, "HEAD", None, asked, 4, a.name(), &call).ok()
}

fn asked_path(v: &Vcs) -> PathBuf {
    v.git_path("flow-asked")
}

/// What the agent has been asked since the last save.
pub fn asked_so_far(v: &Vcs) -> Option<String> {
    std::fs::read_to_string(asked_path(v)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

fn note_prompt(v: &Vcs, prompt: &str) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(asked_path(v))?;
    writeln!(f, "{}", prompt.trim())?;
    Ok(())
}

/// Word the change in a process of its own, so the work finishes even if the agent exits. The
/// turn waits a moment for it, so the sentence is usually there to show, and never much longer.
fn word_apart(v: &Vcs, fired: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut c = Command::new(exe);
    c.arg("-C").arg(&v.root).args(["--out", &v.out, "hook", "word", fired]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let Ok(mut child) = c.spawn() else { return };
    for _ in 0..150 {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A turn ended: describe the change, and return the comment if it says something new.
fn turn_ended(v: &mut Vcs, fired: &str) -> Result<Option<String>> {
    let asked = asked_so_far(v);
    word_apart(v, fired);
    let root = v.root.clone();
    let c = comment::between(v, "HEAD", None, |a, b, r| comment::write_with(a, b, &r, asked, None, &words::Words::load(b, &root)))?;
    if c.is_empty() {
        return Ok(None);
    }
    let text = comment::render(&c, false);
    let seen = v.git_path("flow-shown");
    if std::fs::read_to_string(&seen).ok().as_deref() == Some(text.as_str()) {
        return Ok(None);
    }
    std::fs::write(seen, &text)?;
    Ok(Some(text))
}

fn stdin_json() -> Value {
    serde_json::from_reader(std::io::stdin()).unwrap_or(Value::Null)
}

fn claude_hook(settings: &mut Value, event: &str, command: &str, timeout: u64) -> bool {
    let list = settings["hooks"][event].as_array().cloned().unwrap_or_default();
    let key = command.rsplit(" hook ").next().unwrap_or("");
    let have = list.iter().flat_map(|e| e["hooks"].as_array().cloned().unwrap_or_default()).any(|h| h["command"].as_str().is_some_and(|c| c.ends_with(&format!(" hook {key}"))));
    if have {
        return false;
    }
    let mut list = list;
    list.push(json!({"hooks": [{"type": "command", "command": command, "timeout": timeout}]}));
    settings["hooks"][event] = Value::Array(list);
    true
}

/// Connect this repo to the agents on this machine. Claude Code's hooks live in a settings file
/// that stays on this machine. Codex allows one global program, which is its owner's to set.
pub fn install(v: &Vcs) -> Result<String> {
    let exe = std::env::current_exe()?.display().to_string();
    let mut said = String::new();
    if on_path("claude") {
        let path = v.root.join(".claude/settings.local.json");
        let mut s: Value = match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).with_context(|| format!("{} is not valid JSON, so it was left alone", path.display()))?,
            Err(_) => json!({}),
        };
        if !s["hooks"].is_object() {
            s["hooks"] = json!({});
        }
        let run = |what: &str| format!("\"{exe}\" -C \"$CLAUDE_PROJECT_DIR\" hook {what}");
        let added = claude_hook(&mut s, "UserPromptSubmit", &run("prompt"), 5) | claude_hook(&mut s, "Stop", &run("done"), 30);
        if added {
            std::fs::create_dir_all(path.parent().context("no parent")?)?;
            std::fs::write(&path, serde_json::to_string_pretty(&s)? + "\n")?;
        }
        // `flow save` commits everything, and this file names a path on this machine.
        let exclude = v.git_path("info/exclude");
        let text = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !text.lines().any(|l| l == ".claude/settings.local.json") {
            if let Some(p) = exclude.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&exclude, format!("{text}{}.claude/settings.local.json\n", if text.is_empty() || text.ends_with('\n') { "" } else { "\n" }))?;
        }
        said += if added { "Claude Code: connected for this repo, on this machine.\n" } else { "Claude Code: already connected.\n" };
    }
    if on_path("codex") {
        said += &format!("Codex: add this line to ~/.codex/config.toml (it applies to every repo, and flow ignores folders that are not git repositories):\n  notify = [{exe:?}, \"hook\", \"codex\"]\n");
    }
    if said.is_empty() {
        said = "Neither claude nor codex is installed here. Flow still works, with plain facts.\n".into();
    }
    Ok(said)
}

/// What the agents call. A hook must never get in the way of the agent, so failures stay quiet.
pub fn hook(root: &Path, out: &str, names: crate::names::Names, what: &str, arg: Option<&str>) -> Result<()> {
    if std::env::var_os(INNER).is_some() {
        return Ok(());
    }
    let open = |p: &Path, n: crate::names::Names| Vcs::open(p, out, n);
    match what {
        "prompt" => {
            let v = open(root, names)?;
            if let Some(p) = stdin_json()["prompt"].as_str() {
                note_prompt(&v, p)?;
            }
        }
        "done" => {
            let mut v = open(root, names)?;
            if let Some(text) = turn_ended(&mut v, "claude")? {
                println!("{}", json!({ "systemMessage": format!("Flow:\n{text}") }));
            }
        }
        "word" => {
            let mut v = open(root, names)?;
            // The agent at work is the one its owner is using, unless they chose another.
            let chosen = std::env::var("FLOW_AI").ok().or(crate::config::load(&v.root).ok().and_then(|c| c.ai));
            let agent = pick(&v.root, chosen.as_deref().or(arg));
            let asked = asked_so_far(&v);
            describe(&mut v, agent, asked.as_deref());
        }
        "codex" => {
            let event: Value = serde_json::from_str(arg.unwrap_or("")).unwrap_or(Value::Null);
            if event["type"] != "agent-turn-complete" {
                return Ok(());
            }
            let Some(cwd) = event["cwd"].as_str() else { return Ok(()) };
            let mut v = open(Path::new(cwd), names)?;
            for m in event["input-messages"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                note_prompt(&v, m)?;
            }
            turn_ended(&mut v, "codex")?;
        }
        _ => bail!("unknown hook {what}"),
    }
    Ok(())
}
