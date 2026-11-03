//! Lockfiles, read for what they mean: which package is at which version.

use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const LOCKS: &[&str] = &["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "Cargo.lock", "poetry.lock", "uv.lock", "go.sum", "Gemfile.lock", "composer.lock"];

pub fn is_lock(path: &str) -> bool {
    LOCKS.contains(&path.rsplit('/').next().unwrap_or(path))
}

struct Re {
    pnpm: Regex,
    yarn: Regex,
    gem: Regex,
    toml_name: Regex,
    toml_ver: Regex,
}

fn re() -> &'static Re {
    static R: OnceLock<Re> = OnceLock::new();
    R.get_or_init(|| Re {
        pnpm: Regex::new(r"^\s+/?'?(@?[^@\s:']+)@([0-9][^:\s('@]*)").unwrap(),
        yarn: Regex::new(r#"^"?(@?[^@\s"]+)@"#).unwrap(),
        gem: Regex::new(r"^    ([a-zA-Z0-9_.-]+) \(([^)]+)\)").unwrap(),
        toml_name: Regex::new(r#"^name = "([^"]+)""#).unwrap(),
        toml_ver: Regex::new(r#"^version = "([^"]+)""#).unwrap(),
    })
}

/// name -> version. Several versions of one package are joined, so any change shows.
pub fn parse(path: &str, text: &str) -> BTreeMap<String, String> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |n: &str, v: &str| {
        let e = out.entry(n.to_string()).or_default();
        if !e.iter().any(|x| x == v) {
            e.push(v.to_string());
        }
    };
    let r = re();
    match file {
        "package-lock.json" | "composer.lock" => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(p) = v["packages"].as_object() {
                    for (k, x) in p {
                        let name = k.rsplit("node_modules/").next().unwrap_or(k);
                        if let (false, Some(ver)) = (name.is_empty(), x["version"].as_str()) {
                            add(name, ver);
                        }
                    }
                } else if let Some(p) = v["packages"].as_array() {
                    for x in p {
                        if let (Some(n), Some(ver)) = (x["name"].as_str(), x["version"].as_str()) {
                            add(n, ver);
                        }
                    }
                }
            }
        }
        "pnpm-lock.yaml" => {
            let mut on = false;
            for l in text.lines() {
                if l.trim().is_empty() {
                    continue;
                }
                if !l.starts_with(' ') {
                    on = l.starts_with("packages:");
                    continue;
                }
                if on {
                    if let Some(c) = r.pnpm.captures(l) {
                        add(&c[1], &c[2]);
                    }
                }
            }
        }
        "yarn.lock" => {
            let mut name = String::new();
            for l in text.lines() {
                if let Some(c) = r.yarn.captures(l).filter(|_| !l.starts_with(' ') && !l.starts_with('#')) {
                    name = c[1].to_string();
                } else if let Some(v) = l.trim().strip_prefix("version ") {
                    add(&name, v.trim_matches('"'));
                }
            }
        }
        "go.sum" => {
            for l in text.lines() {
                let p: Vec<&str> = l.split_whitespace().collect();
                if p.len() >= 2 && !p[1].ends_with("/go.mod") {
                    add(p[0], p[1]);
                }
            }
        }
        "Gemfile.lock" => {
            for l in text.lines() {
                if let Some(c) = r.gem.captures(l) {
                    add(&c[1], &c[2]);
                }
            }
        }
        _ => {
            // Cargo.lock, poetry.lock, uv.lock: [[package]] blocks.
            let mut name = String::new();
            for l in text.lines() {
                if let Some(c) = r.toml_name.captures(l) {
                    name = c[1].to_string();
                } else if let Some(c) = r.toml_ver.captures(l) {
                    if !name.is_empty() {
                        add(&name, &c[1]);
                        name.clear();
                    }
                }
            }
        }
    }
    out.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v.join(", "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_the_common_lockfiles() {
        let npm = r#"{"packages":{"":{"name":"app"},"node_modules/stripe":{"version":"14.1.0"},"node_modules/a/node_modules/b":{"version":"2.0.0"}}}"#;
        let p = parse("package-lock.json", npm);
        assert_eq!(p["stripe"], "14.1.0");
        assert_eq!(p["b"], "2.0.0");
        let pnpm = "lockfileVersion: '9.0'\nimporters:\n  .:\n    x: 1\npackages:\n\n  '@scope/x@1.2.3':\n    resolution: {}\n  stripe@14.1.0:\n    resolution: {}\n";
        let p = parse("pnpm-lock.yaml", pnpm);
        assert_eq!(p["@scope/x"], "1.2.3");
        assert_eq!(p["stripe"], "14.1.0");
        let cargo = "[[package]]\nname = \"serde\"\nversion = \"1.0.1\"\n\n[[package]]\nname = \"regex\"\nversion = \"1.2.0\"\n";
        assert_eq!(parse("Cargo.lock", cargo)["regex"], "1.2.0");
        let yarn = "# yarn\n\"@a/b@^1.0.0\", \"@a/b@^1.1.0\":\n  version \"1.2.0\"\n\nleft-pad@1:\n  version \"1.3.0\"\n";
        let p = parse("yarn.lock", yarn);
        assert_eq!(p["@a/b"], "1.2.0");
        assert_eq!(p["left-pad"], "1.3.0");
        assert_eq!(parse("go.sum", "github.com/x/y v1.2.3 h1:abc=\ngithub.com/x/y v1.2.3/go.mod h1:def=\n")["github.com/x/y"], "v1.2.3");
        assert_eq!(parse("Gemfile.lock", "GEM\n  specs:\n    rails (7.1.0)\n      rack (>= 2)\n")["rails"], "7.1.0");
    }
}
