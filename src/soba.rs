//! Soba: AI for a signed-in person on the machine they connected, in place of the agent on this
//! machine. Every prompt here already holds what the model needs, so a run reaches no repo.
//!
//! Soba's SDK is JavaScript, so a small helper does that half: written into `~/.flow` beside an
//! exact, integrity-pinned copy of the SDK, then asked one thing at a time over stdin. Runs are
//! sealed to the person's own machines, so Soba routes them and cannot read them.
//!
//! The key comes from Soba's own CLI, which writes it to `.env.local`. A host's own `SOBA_KEY`
//! wins. Who a run is for comes from `flow serve`'s sign-in, never from a browser: it decides whose
//! machine the run reaches. A command typed in a terminal names nobody, so it uses the local agent.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set by `flow serve` on the work it starts, naming the signed-in person.
pub const USER: &str = "FLOW_SOBA_USER";
/// Starts a refusal's text, so its code survives the trip out of a child process.
const REFUSED: &str = "soba:";
const HELPER: &str = include_str!("soba_helper/helper.mjs");
const PACKAGE: &str = include_str!("soba_helper/package.json");
const LOCK: &str = include_str!("soba_helper/package-lock.json");
/// Part of the folder's name, so a new version installs beside the old one.
const SDK: &str = "0.0.19";
/// The oldest Node the SDK runs on.
const NODE: u32 = 22;

/// `SOBA_KEY` in a file `@soba-so/cli link` wrote. Nothing else in it is read, so a production key
/// beside it stays out of this process.
pub fn key_in(file: &str) -> Option<String> {
    file.lines().find_map(|l| l.trim().strip_prefix("SOBA_KEY=")).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).filter(|k| !k.is_empty())
}

/// Called before any thread starts. Work this server starts inherits the key with no file of its
/// own. Only the folder flow started in is read: a project's `.env.local` is that project's, and
/// its key would be another app's.
pub fn load_key(named: Option<&Path>, started_in: &Path) -> Result<()> {
    if std::env::var_os("SOBA_KEY").is_none() {
        read_key(named, started_in)?;
    }
    if ready() {
        prepare()?;
    }
    Ok(())
}

fn read_key(named: Option<&Path>, started_in: &Path) -> Result<()> {
    let path = named.map_or_else(|| started_in.join(".env.local"), Path::to_path_buf);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) if named.is_none() => return Ok(()),
        Err(e) => bail!("could not read {}: {e}. Run `npx @soba-so/cli link` to write it", path.display()),
    };
    match key_in(&text) {
        Some(k) => std::env::set_var("SOBA_KEY", k),
        None if named.is_some() => bail!("{} has no SOBA_KEY. Run `npx @soba-so/cli link` to write it", path.display()),
        None => {}
    }
    Ok(())
}

/// The code and sentence of a refusal, wherever it sits in what a failed task said.
pub fn refusal(said: &str) -> Option<(&str, &str)> {
    let rest = &said[said.find(REFUSED)? + REFUSED.len()..];
    rest.split_once(": ")
}

/// Whether Soba can be used here at all.
pub fn ready() -> bool {
    std::env::var("SOBA_KEY").is_ok_and(|k| !k.trim().is_empty())
}

/// Who this work is for, when Soba is set up and someone was named.
pub fn person() -> Option<String> {
    std::env::var(USER).ok().filter(|u| ready() && !u.is_empty())
}

fn home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("FLOW_SOBA_HOME") {
        return Ok(PathBuf::from(h));
    }
    Ok(PathBuf::from(std::env::var("HOME").context("no home folder")?).join(".flow").join(format!("soba-{SDK}")))
}

/// Install the helper once, up front, so a missing Node stops the server rather than a run.
pub fn prepare() -> Result<()> {
    let need = || format!("Soba needs Node {NODE} or newer, from nodejs.org");
    let out = Command::new("node").arg("--version").output().with_context(need)?;
    let version = String::from_utf8_lossy(&out.stdout);
    if version.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse::<u32>().ok()).is_none_or(|m| m < NODE) {
        bail!("{}. This is {}", need(), version.trim());
    }
    let dir = home()?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    std::fs::write(dir.join("helper.mjs"), HELPER)?;
    if dir.join("node_modules/@soba-so/sdk/package.json").exists() {
        return Ok(());
    }
    std::fs::write(dir.join("package.json"), PACKAGE)?;
    std::fs::write(dir.join("package-lock.json"), LOCK)?;
    let done = Command::new("npm").args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]).current_dir(&dir).stdin(Stdio::null()).output().context("could not start npm")?;
    if !done.status.success() {
        bail!("could not install Soba's SDK: {}", String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or(""));
    }
    Ok(())
}

fn call(ask: &Value) -> Result<Value> {
    let dir = home()?;
    let mut c = Command::new("node").arg(dir.join("helper.mjs")).current_dir(&dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("could not start node")?;
    c.stdin.take().context("no stdin")?.write_all(ask.to_string().as_bytes())?;
    let done = c.wait_with_output()?;
    serde_json::from_slice(&done.stdout).map_err(|_| anyhow!("Soba's helper stopped: {}", String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or("")))
}

/// What lets a browser act for this person. Status and body are passed on as they came.
pub fn session(login: &str) -> Result<(u16, String)> {
    let v = call(&json!({ "op": "session", "login": login }))?;
    if let Some(e) = v["failed"].as_str() {
        bail!("could not reach Soba: {e}");
    }
    Ok((v["status"].as_u64().context("no status")? as u16, v["body"].as_str().unwrap_or("").to_string()))
}

/// One answer shaped like `schema`. Runs have no schema of their own, so the prompt asks for it,
/// and what comes back is checked. The model is `auto`: a named one refuses what a machine lacks.
pub fn ask(login: &str, prompt: &str, schema: &Value) -> Result<Value> {
    let prompt = format!("{prompt}\n\nAnswer with one JSON object and nothing else, no code fences. It must match this JSON Schema:\n{schema}");
    let v = call(&json!({ "op": "run", "login": login, "prompt": prompt }))?;
    if let Some(r) = v.get("refused") {
        bail!("{REFUSED}{}: {}", r["code"].as_str().unwrap_or("refused"), r["message"].as_str().unwrap_or("Soba refused this run"));
    }
    if let Some(e) = v["failed"].as_str() {
        bail!("could not reach Soba: {e}");
    }
    answer(v["text"].as_str().unwrap_or(""), schema)
}

/// The object in a model's text, with every field the schema requires.
fn answer(text: &str, schema: &Value) -> Result<Value> {
    let (from, to) = text.find('{').zip(text.rfind('}')).ok_or_else(|| anyhow!("the run did not answer in the expected shape"))?;
    let v: Value = serde_json::from_str(&text[from..=to]).context("the run did not answer in the expected shape")?;
    let missing = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).find(|k| v.get(*k).is_none());
    if let Some(k) = missing {
        bail!("the run's answer has no {k}");
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_development_key_is_read_from_the_cli_file() {
        let file = "# Soba\nSOBA_PROD_KEY=sk_soba_prod.x\nSOBA_KEY=\"sk_soba_dev.y\"\nSOBA_APP_ID=abc\n";
        assert_eq!(key_in(file).as_deref(), Some("sk_soba_dev.y"));
        assert_eq!(key_in("SOBA_PROD_KEY=sk_soba_prod.x\nSOBA_KEY=\n"), None);
    }

    #[test]
    fn an_answer_has_to_have_the_shape_asked_for() {
        let schema = json!({ "required": ["stops"] });
        assert_eq!(answer("Sure:\n```json\n{\"stops\":[]}\n```", &schema).unwrap(), json!({ "stops": [] }));
        for bad in ["no json at all", "{\"other\":1}", "{broken}"] {
            assert!(answer(bad, &schema).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_refusal_keeps_its_code_through_a_failed_task() {
        assert_eq!(refusal("  failed: Pay: soba:no_compute: No compute for usr_1."), Some(("no_compute", "No compute for usr_1.")));
        assert_eq!(refusal("could not start `claude`"), None);
    }
}
