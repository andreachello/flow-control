//! Pull requests, read through the GitHub command line, so the person's own sign-in decides what can be seen.
//! Nothing here is ever fetched into or written to their repository.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// GitHub answers a list of this many at once, and older ones are rarely what anyone is looking for.
const MOST: &str = "100";
const LISTED: &str = "number,title,state,isDraft,author,createdAt,mergedAt,closedAt,baseRefName,headRefName,changedFiles,url";
const OPENED: &str = "number,title,state,isDraft,author,createdAt,mergedAt,closedAt,baseRefName,headRefName,changedFiles,url,body,commits";

fn gh(root: &Path, args: &[&str]) -> Result<Value> {
    let o = Command::new("gh").args(args).current_dir(root).output().context("the gh command is not installed")?;
    if !o.status.success() {
        bail!("{}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}

/// What a person sees, with the state a pull request is really in: GitHub calls a draft open.
fn plain(p: &Value) -> Value {
    let state = match (p["state"].as_str(), p["isDraft"].as_bool()) {
        (Some("OPEN"), Some(true)) => "draft",
        (Some("OPEN"), _) => "open",
        (Some("MERGED"), _) => "merged",
        _ => "closed",
    };
    let date = |k: &str| p[k].as_str().map(|d| d.chars().take(10).collect::<String>());
    json!({ "number": p["number"], "title": p["title"], "state": state, "author": p["author"]["login"], "opened": date("createdAt"), "ended": date("mergedAt").or(date("closedAt")),
        "base": p["baseRefName"], "head": p["headRefName"], "files": p["changedFiles"], "url": p["url"] })
}

/// A project off GitHub is an ordinary answer, not a failure of the page.
pub fn list(root: &Path) -> Value {
    match gh(root, &["pr", "list", "--state", "all", "--limit", MOST, "--json", LISTED]) {
        Ok(Value::Array(a)) => json!({ "linked": true, "pulls": a.iter().map(plain).collect::<Vec<_>>() }),
        Ok(_) => json!({ "linked": false, "pulls": [] }),
        Err(e) => {
            eprintln!("flow serve: pull requests: {e}");
            json!({ "linked": false, "pulls": [] })
        }
    }
}

/// One pull request, and the commits it holds, newest last.
pub fn one(root: &Path, number: u64) -> Result<(Value, Vec<String>)> {
    let p = gh(root, &["pr", "view", &number.to_string(), "--json", OPENED])?;
    let ids = p["commits"].as_array().into_iter().flatten().filter_map(|c| c["oid"].as_str()).map(str::to_string).collect();
    let mut shown = plain(&p);
    shown["body"] = p["body"].clone();
    Ok((shown, ids))
}
