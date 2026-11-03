//! Entry points by convention, and flows as the depth-first walk from each one.

use crate::parse::Role;
use crate::repo::{Graph, Repo, Res, SRef, Unres};
use crate::util::{dirname, slug};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub kind: &'static str,
    pub at: SRef,
    /// What the framework runs around the entry without any line of code calling it:
    /// layouts, loading and error boundaries, generateMetadata. (slice, label)
    pub framework: Vec<(SRef, String)>,
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

fn route_of(dir: &str) -> String {
    let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')')) && !s.starts_with('@')).collect();
    format!("/{}", segs.join("/"))
}

const PAGE_EXPORTS: &[&str] = &["generateMetadata", "generateStaticParams", "generateViewport"];
const AROUND_PAGE: &[&str] = &["layout", "template", "loading", "error", "not-found", "default"];
const METADATA_FILES: &[&str] = &["robots", "sitemap", "manifest", "opengraph-image", "twitter-image", "icon", "apple-icon"];
const CONV_EXTS: &[&str] = &[".tsx", ".ts", ".jsx", ".js"];

fn conv_file(repo: &Repo, dir: &str, stem: &str) -> Option<usize> {
    CONV_EXTS.iter().find_map(|e| repo.file_idx(&format!("{}{stem}{e}", if dir.is_empty() { String::new() } else { format!("{dir}/") })))
}

/// Everything Next.js runs for a page besides the page itself, outermost layout first.
fn around_page(repo: &Repo, page_file: usize, app_root: &str) -> Vec<(SRef, String)> {
    let mut out = vec![];
    for name in PAGE_EXPORTS {
        if let Res::Slice(r) = repo.resolve_export(page_file, name, 0) {
            out.push((r, format!("framework: {name}")));
        }
    }
    let page_dir = dirname(&repo.files[page_file].path).to_string();
    let rel = page_dir.strip_prefix(app_root).unwrap_or("").trim_start_matches('/');
    let mut dir = app_root.to_string();
    let mut dirs = vec![dir.clone()];
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        dir = format!("{dir}/{seg}");
        dirs.push(dir.clone());
    }
    for d in &dirs {
        for st in AROUND_PAGE {
            let Some(fi) = conv_file(repo, d, st) else { continue };
            if let Res::Slice(r) = repo.resolve_export(fi, "default", 0) {
                out.push((r, format!("framework: {st}")));
            }
            if *st == "layout" {
                for name in PAGE_EXPORTS {
                    if let Res::Slice(r) = repo.resolve_export(fi, name, 0) {
                        out.push((r, format!("framework: layout {name}")));
                    }
                }
            }
        }
    }
    out
}

fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}

/// Next.js App Router routes and pages, Pages Router, server actions, middleware, plus flow.toml entries.
pub fn entries(repo: &Repo) -> (Vec<Entry>, Vec<String>) {
    let mut out: Vec<Entry> = vec![];
    let mut problems = vec![];
    let push = |out: &mut Vec<Entry>, id: String, kind: &'static str, res: Res| {
        if let Res::Slice(r) = res {
            if !out.iter().any(|e| e.at == r) {
                out.push(Entry { id, kind, at: r, framework: vec![] });
            }
        }
    };
    for (fi, f) in repo.files.iter().enumerate() {
        let p = f.path.strip_prefix("src/").unwrap_or(&f.path);
        let st = stem(p);
        if let Some(rest) = p.strip_prefix("app/") {
            let route = route_of(dirname(rest));
            let app_root = if f.path.starts_with("src/") { "src/app" } else { "app" };
            if st == "route" {
                for m in METHODS {
                    push(&mut out, format!("{m} {route}"), "route", repo.resolve_export(fi, m, 0));
                }
            } else if st == "page" {
                push(&mut out, format!("page {route}"), "page", repo.resolve_export(fi, "default", 0));
                if let Some(e) = out.last_mut().filter(|e| e.at.0 == fi) {
                    e.framework = around_page(repo, fi, app_root);
                }
            } else if METADATA_FILES.contains(&st) {
                let at = if route == "/" { format!("/{st}") } else { format!("{route}/{st}") };
                push(&mut out, format!("metadata {at}"), "metadata", repo.resolve_export(fi, "default", 0));
            } else if st == "global-error" {
                push(&mut out, "error page".into(), "page", repo.resolve_export(fi, "default", 0));
            }
        } else if let Some(rest) = p.strip_prefix("pages/") {
            if !st.starts_with('_') {
                let mut route = route_of(&rest[..rest.rfind('.').unwrap_or(rest.len())]);
                if let Some(r) = route.strip_suffix("/index") {
                    route = if r.is_empty() { "/".into() } else { r.into() };
                }
                let kind = if rest.starts_with("api/") { "api" } else { "page" };
                push(&mut out, format!("{kind} {route}"), kind, repo.resolve_export(fi, "default", 0));
            }
        } else if (p == "middleware.ts" || p == "middleware.js") && !f.path.contains("/node_modules/") {
            let res = match repo.resolve_export(fi, "middleware", 0) {
                Res::Slice(r) => Res::Slice(r),
                _ => repo.resolve_export(fi, "default", 0),
            };
            push(&mut out, "middleware".into(), "middleware", res);
        }
        if f.directives.iter().any(|d| d == "use server") {
            for (si, s) in f.slices.iter().enumerate() {
                if s.exported && s.role == Role::Function {
                    push(&mut out, format!("action {}", s.name), "action", Res::Slice((fi, si)));
                }
            }
        }
    }
    universal_entries(repo, &mut out);
    for e in &repo.cfg.entry {
        match repo.file_idx(&e.file) {
            Some(fi) => match repo.resolve_export(fi, &e.export, 0) {
                Res::Slice(r) => push(&mut out, e.id.clone().unwrap_or_else(|| format!("entry {}", e.export)), "configured", Res::Slice(r)),
                _ => problems.push(format!("flow.toml entry {}#{} does not exist", e.file, e.export)),
            },
            None => problems.push(format!("flow.toml entry file {} is not in the repo", e.file)),
        }
    }
    finish(&mut out);
    (out, problems)
}

/// When nothing in a repo looks like an entry point (a library, scripts, contracts, a framework flow
/// has no rule for), the functions nothing else calls are where everything starts.
pub fn roots(repo: &Repo, g: &Graph) -> Vec<Entry> {
    let called: HashSet<SRef> = g.edges.values().flatten().map(|e| e.to).collect();
    let mut all = vec![];
    for (fi, f) in repo.files.iter().enumerate() {
        if crate::output::is_test_file(&f.path) {
            continue;
        }
        for (si, s) in f.slices.iter().enumerate().skip(1) {
            let r = (fi, si);
            let class_called = s.name.rsplit_once('.').and_then(|(c, _)| f.by_name.get(c)).map(|&i| called.contains(&(fi, i))).unwrap_or(false);
            if s.role.runs() && !called.contains(&r) && !class_called {
                all.push(r);
            }
        }
    }
    let busy: Vec<SRef> = all.iter().copied().filter(|r| g.edges.get(r).map(|e| !e.is_empty()).unwrap_or(false)).collect();
    let pick = if busy.is_empty() { all } else { busy };
    let mut out: Vec<Entry> = pick.into_iter().map(|r| Entry { id: format!("start {}", repo.slice(r).name), kind: "start", at: r, framework: vec![] }).collect();
    finish(&mut out);
    out
}

fn finish(out: &mut [Entry]) {
    // Unique ids, deterministic order.
    out.sort_by(|a, b| a.id.cmp(&b.id).then(a.at.cmp(&b.at)));
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for e in out.iter_mut() {
        let n = seen.entry(e.id.clone()).or_default();
        *n += 1;
        if *n > 1 {
            e.id = format!("{} ({})", e.id, n);
        }
    }
}

/// Entry points that look the same in most languages and frameworks.
fn universal_entries(repo: &Repo, out: &mut Vec<Entry>) {
    use regex::Regex;
    use std::sync::OnceLock;
    static DECOR: OnceLock<Vec<Regex>> = OnceLock::new();
    let decor = DECOR.get_or_init(|| {
        vec![
            // FastAPI, Flask, Starlette, Sanic, Bottle, NestJS-like: @app.get("/x"), @router.post('/x')
            Regex::new(r#"^\s*@[\w.]*?\b(get|post|put|patch|delete|head|options|route|api_route|websocket)\(\s*["']([^"']*)"#).unwrap(),
            // Spring, Micronaut, NestJS: @GetMapping("/x"), @Get("/x"), @RequestMapping(value = "/x")
            Regex::new(r#"^\s*@(Get|Post|Put|Delete|Patch|Request)(?:Mapping)?\(\s*(?:value\s*=\s*|path\s*=\s*)?["']([^"']*)"#).unwrap(),
            // Rust (actix, rocket, axum macros): #[get("/x")]
            Regex::new(r#"^\s*#\[(get|post|put|delete|patch)\(\s*"([^"]*)"#).unwrap(),
        ]
    });
    const VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];
    let push = |out: &mut Vec<Entry>, id: String, kind: &'static str, at: SRef| {
        if !out.iter().any(|e| e.at == at) {
            out.push(Entry { id, kind, at, framework: vec![] });
        }
    };
    for (fi, f) in repo.files.iter().enumerate() {
        if crate::output::is_test_file(&f.path) {
            continue;
        }
        for (si, s) in f.slices.iter().enumerate().skip(1) {
            if !f.grammar {
                // Decorators and attributes written above the block.
                for &r in s.lines.iter().filter(|r| !f.quoted.contains(r)) {
                    for re in decor.iter() {
                        if let Some(c) = re.captures(&f.lines[r]) {
                            let verb = c[1].to_lowercase();
                            let method = if VERBS.contains(&verb.as_str()) { verb.to_uppercase() } else { "ROUTE".into() };
                            let path = if c[2].starts_with('/') { c[2].to_string() } else { format!("/{}", &c[2]) };
                            push(out, format!("{method} {path}"), "route", (fi, si));
                        }
                    }
                }
                if s.name == "main" && s.role == Role::Function {
                    push(out, format!("program {}", if dirname(&f.path).is_empty() { f.path.clone() } else { dirname(&f.path).to_string() }), "program", (fi, si));
                }
                if s.name == "__main__" {
                    push(out, format!("script {}", f.path), "program", (fi, si));
                }
            }
            // Registration calls: app.get("/x", handler), http.HandleFunc("/x", h), path("x/", view).
            for c in &s.calls {
                let verb = match &c.callee {
                    crate::parse::Callee::Member(_, p) | crate::parse::Callee::Method(_, p) | crate::parse::Callee::Ident(p) => p.to_lowercase(),
                    _ => continue,
                };
                let registers = VERBS.contains(&verb.as_str()) || ["handlefunc", "handle", "route", "add_url_rule", "path", "re_path", "url", "add_route", "add_api_route", "all", "use"].contains(&verb.as_str());
                let (Some(crate::parse::Arg::Str(path)), Some(h)) = (c.args.first(), c.args.get(1)) else { continue };
                if !registers || path.is_empty() || path.contains(' ') || path.len() > 120 {
                    continue;
                }
                if !(path.starts_with('/') || verb == "path" || verb == "re_path" || verb == "url") {
                    continue;
                }
                let res = match h {
                    crate::parse::Arg::Ident(n) => repo.resolve_ident(fi, n, &s.locals),
                    crate::parse::Arg::Member(o, p) if !f.grammar => repo.member_in_lang(fi, o, p),
                    _ => continue,
                };
                if let Res::Slice(r) = res {
                    if repo.slice(r).role.runs() {
                        let method = if VERBS.contains(&verb.as_str()) { verb.to_uppercase() } else { "ROUTE".into() };
                        let path = if path.starts_with('/') { path.clone() } else { format!("/{path}") };
                        push(out, format!("{method} {path}"), "route", r);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub at: SRef,
    pub depth: usize,
    pub via: String,
}

#[derive(Debug, Clone)]
pub struct Flow {
    pub id: String,
    pub slug: String,
    pub kind: &'static str,
    pub entry: SRef,
    pub steps: Vec<Step>,
    pub outside: BTreeSet<String>,
    pub unresolved: Vec<(SRef, Unres)>,
    pub not_followed: usize,
}

pub fn build(repo: &Repo, g: &Graph, entries: &[Entry]) -> Vec<Flow> {
    let mut slugs: HashSet<String> = HashSet::new();
    entries
        .iter()
        .map(|e| {
            let mut steps = vec![];
            let mut seen: HashSet<SRef> = HashSet::new();
            let mut stack: Vec<(SRef, usize, String)> = e.framework.iter().rev().map(|(r, l)| (*r, 1, l.clone())).collect();
            stack.push((e.at, 0, "entry".into()));
            while let Some((r, depth, via)) = stack.pop() {
                if !seen.insert(r) {
                    continue;
                }
                steps.push(Step { at: r, depth, via });
                if let Some(es) = g.edges.get(&r) {
                    for ed in es.iter().rev() {
                        if !seen.contains(&ed.to) {
                            stack.push((ed.to, depth + 1, ed.kind.label()));
                        }
                    }
                }
            }
            let mut outside = BTreeSet::new();
            let mut unresolved = vec![];
            let mut not_followed = 0;
            for s in &steps {
                outside.extend(g.external.get(&s.at).cloned().unwrap_or_default());
                for u in g.unresolved.get(&s.at).cloned().unwrap_or_default() {
                    unresolved.push((s.at, u));
                }
                not_followed += g.not_followed.get(&s.at).copied().unwrap_or(0);
            }
            let mut sl = slug(&e.id);
            let base = sl.clone();
            let mut k = 2;
            while !slugs.insert(sl.clone()) {
                sl = format!("{base}-{k}");
                k += 1;
            }
            let _ = repo;
            Flow { id: e.id.clone(), slug: sl, kind: e.kind, entry: e.at, steps, outside, unresolved, not_followed }
        })
        .collect()
}
