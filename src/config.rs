//! flow.toml: the few things a person tells the lifter that it cannot prove on its own.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Path prefixes to leave out, relative to the repo root.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Extra journeys the conventions do not find (jobs, webhooks, CLIs).
    #[serde(default)]
    pub entry: Vec<EntryCfg>,
    /// Owners for lines the parser will not guess, matched by the statement's first line.
    #[serde(default)]
    pub assign: Vec<AssignCfg>,
    #[serde(default)]
    pub rules: Rules,
    /// Commands that run a journey for real and print what happened.
    #[serde(default)]
    pub scenario: Vec<ScenarioCfg>,
    /// Which agent may word changes: "claude", "codex" or "off". Left out, whichever is installed.
    pub ai: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ScenarioCfg {
    pub name: String,
    /// The journey it exercises. Left out, it runs for every merge.
    pub journey: Option<String>,
    pub run: String,
    pub timeout: Option<u64>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EntryCfg {
    pub file: String,
    pub export: String,
    pub id: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AssignCfg {
    pub file: String,
    pub line: String,
    pub to: String,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Event wiring: `register("evt", handler)` plus `emit("evt")` means emit reaches handler.
    #[serde(default)]
    pub event: Vec<EventRule>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EventRule {
    pub register: String,
    pub emit: String,
}

pub fn load(root: &Path) -> Result<Config> {
    let p = root.join("flow.toml");
    if !p.exists() {
        return Ok(Config::default());
    }
    parse(&std::fs::read_to_string(&p)?)
}

pub fn parse(s: &str) -> Result<Config> {
    toml::from_str(s).context("reading flow.toml")
}
