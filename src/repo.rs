//! Load a repo and resolve every call to a slice, a package, a builtin, or a loud unresolved entry.

use crate::config::{self, Config};
use crate::parse::{self, Arg, CallKind, Callee, ImportName, ParsedFile, Role, SUPPORT};
use crate::util::{dirname, normalize, strip_jsonc};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// (file index, slice index)
pub type SRef = (usize, usize);

#[derive(Debug, Clone)]
pub enum Res {
    Slice(SRef),
    Namespace(usize),
    External(String),
    Builtin,
    Local,
    NotFollowed,
    /// Linked by matching the name alone: nothing confirms it.
    Guess(SRef),
    Missing(String),
}

pub struct TsPaths {
    base: String,
    paths: Vec<(String, Vec<String>)>,
}

pub struct Repo {
    pub root: PathBuf,
    pub out: String,
    pub cfg: Config,
    pub files: Vec<ParsedFile>,
    index: HashMap<String, usize>,
    ts: TsPaths,
    /// (language, name) -> top-level definitions, for files read without a grammar.
    names: HashMap<(String, String), Vec<SRef>>,
    /// (language, file stem or folder name) -> files: `billing.charge()` in Python or Go.
    modules: HashMap<(String, String), Vec<usize>>,
    /// File stem or folder name -> files, for matching module paths.
    by_stem: HashMap<String, Vec<usize>>,
    /// name -> version, from the repo's lockfiles.
    pub packages: BTreeMap<String, String>,
}

const EXTS: &[&str] = &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

const BUILTINS: &[&str] = &[
    "fetch", "setTimeout", "clearTimeout", "setInterval", "clearInterval", "setImmediate", "queueMicrotask",
    "structuredClone", "parseInt", "parseFloat", "isNaN", "isFinite", "encodeURIComponent", "decodeURIComponent",
    "encodeURI", "decodeURI", "String", "Number", "Boolean", "Symbol", "BigInt", "Array", "Object", "Promise",
    "Date", "RegExp", "Error", "TypeError", "RangeError", "SyntaxError", "AggregateError", "Map", "Set", "WeakMap",
    "WeakSet", "WeakRef", "URL", "URLSearchParams", "Response", "Request", "Headers", "FormData", "Blob", "File",
    "TextEncoder", "TextDecoder", "AbortController", "require", "atob", "btoa", "alert", "confirm", "prompt",
    "requestAnimationFrame", "cancelAnimationFrame", "requestIdleCallback", "eval", "Function", "Proxy",
    "ReadableStream", "WritableStream", "TransformStream", "WebSocket", "Event", "CustomEvent", "EventSource",
    "MutationObserver", "ResizeObserver", "IntersectionObserver", "Image", "Audio", "Buffer", "Uint8Array",
    "Int32Array", "Float32Array", "ArrayBuffer", "DataView", "DOMParser", "XMLHttpRequest", "Worker",
    "BroadcastChannel", "MessageChannel", "Notification", "getComputedStyle", "matchMedia", "scrollTo",
    "AbortSignal", "CompressionStream", "DecompressionStream", "PerformanceObserver", "HTMLElement", "Element",
    "KeyboardEvent", "MouseEvent", "PointerEvent", "Node", "NodeFilter", "Range", "Selection", "ClipboardItem",
    "FileReader", "OffscreenCanvas", "ImageData", "Path2D", "DOMRect", "Float64Array", "Uint8ClampedArray",
    "Uint16Array", "Uint32Array", "Int8Array", "Int16Array", "BigInt64Array", "BigUint64Array", "EvalError",
    "URIError", "ReferenceError", "SharedArrayBuffer", "FinalizationRegistry", "Iterator", "escape", "unescape",
];

const GLOBAL_OBJECTS: &[&str] = &[
    "console", "Math", "JSON", "Object", "Array", "Promise", "Number", "String", "Date", "Reflect", "Intl",
    "Symbol", "BigInt", "process", "window", "document", "navigator", "globalThis", "self", "localStorage",
    "sessionStorage", "crypto", "performance", "location", "history", "Buffer", "URL", "Response", "Atomics",
    "WebAssembly", "Error", "Proxy", "Set", "Map", "module", "exports", "screen", "caches", "indexedDB",
];

pub fn is_source(rel: &str) -> bool {
    EXTS.iter().any(|e| rel.ends_with(e)) && !rel.ends_with(".d.ts") && !rel.ends_with(".min.js")
}

fn read_tsconfig(text: Option<String>) -> TsPaths {
    let mut t = TsPaths { base: String::new(), paths: vec![] };
    let Some(s) = text else { return t };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&s)) else { return t };
    let co = &v["compilerOptions"];
    if let Some(b) = co["baseUrl"].as_str() {
        t.base = normalize(b);
    }
    if let Some(p) = co["paths"].as_object() {
        let mut keys: Vec<&String> = p.keys().collect();
        keys.sort();
        for k in keys {
            let targets = p[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            t.paths.push((k.clone(), targets));
        }
    }
    t
}

/// Methods that only exist on built-in values: lists, text, numbers, dates, maps, promises.
const BUILTIN_METHODS: &[&str] = &[
    "map", "filter", "forEach", "reduce", "reduceRight", "some", "every", "find", "findIndex", "findLast", "findLastIndex", "includes",
    "indexOf", "lastIndexOf", "join", "push", "pop", "shift", "unshift", "slice", "splice", "concat", "sort", "toSorted", "reverse",
    "toReversed", "flat", "flatMap", "fill", "keys", "values", "entries", "at", "replace", "replaceAll", "split", "trim", "trimStart",
    "trimEnd", "startsWith", "endsWith", "toLowerCase", "toUpperCase", "toLocaleLowerCase", "toLocaleUpperCase", "padStart", "padEnd",
    "charAt", "charCodeAt", "codePointAt", "match", "matchAll", "search", "substring", "substr", "localeCompare", "normalize", "repeat",
    "toFixed", "toPrecision", "toString", "toLocaleString", "valueOf", "getTime", "getDate", "getDay", "getMonth", "getFullYear",
    "getHours", "getMinutes", "getSeconds", "getUTCDate", "getUTCDay", "getUTCMonth", "getUTCFullYear", "getUTCHours", "toISOString",
    "toLocaleDateString", "toLocaleTimeString", "toDateString", "toJSON", "setDate", "setMonth", "setFullYear", "setHours",
    "setUTCDate", "setMinutes", "setSeconds", "setTime", "then", "catch", "finally", "has", "add", "clear", "test", "exec",
    "preventDefault", "stopPropagation", "focus", "blur", "click", "scrollIntoView", "getBoundingClientRect", "querySelector",
    "querySelectorAll", "addEventListener", "removeEventListener", "contains", "closest", "setAttribute", "getAttribute",
    "removeAttribute", "appendChild", "remove", "json", "text", "arrayBuffer", "formData", "blob", "bind", "call", "apply",
];

/// Methods that write the table or collection passed as their first argument (Drizzle, Knex, Kysely style).
const WRITE_METHODS: &[&str] = &["insert", "update", "delete", "insertInto", "deleteFrom", "upsert"];

pub enum Module {
    Repo(usize),
    Package(String),
    Missing,
}

impl Repo {
    pub fn load(root: &Path, out: &str) -> Result<Repo> {
        Repo::from_source(&crate::source::Source::Dir(root.to_path_buf()), out)
    }

    pub fn from_source(src: &crate::source::Source, out: &str) -> Result<Repo> {
        Repo::from_source_opts(src, out, false)
    }

    pub fn from_source_opts(src: &crate::source::Source, out: &str, no_grammars: bool) -> Result<Repo> {
        let cfg = match src.read("flow.toml") {
            Some(t) => config::parse(&t)?,
            None => Config::default(),
        };
        let ts = read_tsconfig(src.read("tsconfig.json"));
        let out_norm = normalize(out);
        let all_paths = src.paths()?;
        let paths: Vec<String> = all_paths
            .iter()
            .cloned()
            .filter(|rel| {
                crate::universal::readable(rel)
                    && !rel.ends_with(".d.ts")
                    && !rel.starts_with(&format!("{out_norm}/"))
                    && rel != "flow.names.json"
                    && !cfg.ignore.iter().any(|p| rel.starts_with(p.as_str()))
            })
            .collect();
        let mut files = vec![];
        for (rel, text) in src.read_many(&paths)? {
            // Not text, or too big to be written by hand: version control keeps it, the reader skips it.
            let Some(text) = text else { continue };
            if text.len() > 30_000_000 || text.contains('\0') {
                continue;
            }
            let assigns: Vec<(String, String)> = cfg.assign.iter().filter(|a| a.file == rel).map(|a| (a.line.clone(), a.to.clone())).collect();
            files.push(if no_grammars { crate::universal::parse(&rel, &text, &assigns) } else { parse::parse_any(&rel, &text, &assigns)? });
        }
        // Lockfiles are not code, but what they mean matters: which package is at which version.
        let locks: Vec<String> = all_paths.iter().filter(|p| crate::packages::is_lock(p)).cloned().collect();
        let mut packages = BTreeMap::new();
        for (p, text) in src.read_many(&locks)? {
            if let Some(t) = text {
                packages.extend(crate::packages::parse(&p, &t));
            }
        }
        let index = files.iter().enumerate().map(|(i, f)| (f.path.clone(), i)).collect();
        let mut names: HashMap<(String, String), Vec<SRef>> = HashMap::new();
        let mut modules: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (fi, f) in files.iter().enumerate() {
            if f.grammar {
                continue;
            }
            let file = f.path.rsplit('/').next().unwrap_or(&f.path);
            by_stem.entry(file.split('.').next().unwrap_or(file).to_string()).or_default().push(fi);
            if let Some((stem, _)) = file.rsplit_once('.') {
                if stem.contains('.') {
                    by_stem.entry(stem.to_string()).or_default().push(fi);
                }
            }
            let mut dirs = crate::util::dirname(&f.path).rsplit('/');
            for d in [dirs.next(), dirs.next()].into_iter().flatten() {
                if !d.is_empty() {
                    by_stem.entry(d.to_string()).or_default().push(fi);
                }
            }
            for (si, s) in f.slices.iter().enumerate().skip(1) {
                // Only top-level names: a method's short name never reaches across files by itself.
                names.entry((f.lang.clone(), s.name.clone())).or_default().push((fi, si));
            }
            let stem = f.path.rsplit('/').next().unwrap_or(&f.path).split('.').next().unwrap_or("").to_string();
            modules.entry((f.lang.clone(), stem)).or_default().push(fi);
            if let Some(dir) = crate::util::dirname(&f.path).rsplit('/').next().filter(|d| !d.is_empty()) {
                modules.entry((f.lang.clone(), dir.to_string())).or_default().push(fi);
            }
        }
        Ok(Repo { root: src.root().to_path_buf(), out: out_norm, cfg, files, index, ts, names, modules, by_stem, packages })
    }

    /// Method names defined by any class in the repo.
    pub fn class_methods(&self) -> std::collections::HashSet<String> {
        self.files.iter().flat_map(|f| f.slices.iter().flat_map(|s| s.methods.iter().cloned())).collect()
    }

    pub fn file_idx(&self, path: &str) -> Option<usize> {
        self.index.get(path).copied()
    }

    pub fn sid(&self, r: SRef) -> String {
        format!("{}#{}", self.files[r.0].path, self.files[r.0].slices[r.1].name)
    }

    pub fn slice(&self, r: SRef) -> &parse::Slice {
        &self.files[r.0].slices[r.1]
    }

    fn try_file(&self, p: &str) -> Option<usize> {
        let p = normalize(p);
        let mut cands = vec![p.clone()];
        for e in EXTS {
            cands.push(format!("{p}{e}"));
        }
        for e in EXTS {
            cands.push(format!("{p}/index{e}"));
        }
        // ESM TypeScript writes "./x.js" for "./x.ts".
        for (js, ts) in [(".js", ".ts"), (".js", ".tsx"), (".jsx", ".tsx"), (".mjs", ".mts")] {
            if let Some(stem) = p.strip_suffix(js) {
                cands.push(format!("{stem}{ts}"));
            }
        }
        cands.iter().find_map(|c| self.index.get(c).copied())
    }

    pub fn resolve_module(&self, from: usize, spec: &str) -> Module {
        if spec.starts_with('.') {
            let base = format!("{}/{}", dirname(&self.files[from].path), spec);
            return self.try_file(&base).map(Module::Repo).unwrap_or(Module::Missing);
        }
        for (pat, targets) in &self.ts.paths {
            let rest = match pat.strip_suffix('*') {
                Some(prefix) => spec.strip_prefix(prefix),
                None => (spec == pat).then_some(""),
            };
            let Some(rest) = rest else { continue };
            for t in targets {
                let cand = format!("{}/{}", self.ts.base, t.replace('*', rest));
                if let Some(i) = self.try_file(&cand) {
                    return Module::Repo(i);
                }
            }
        }
        if !self.ts.base.is_empty() || self.ts.paths.is_empty() {
            if let Some(i) = self.try_file(&format!("{}/{}", self.ts.base, spec)) {
                if !spec.contains(':') {
                    return Module::Repo(i);
                }
            }
        }
        let spec = spec.strip_prefix("node:").unwrap_or(spec);
        let mut parts = spec.split('/');
        let first = parts.next().unwrap_or(spec);
        let pkg = if first.starts_with('@') { format!("{}/{}", first, parts.next().unwrap_or("")) } else { first.to_string() };
        Module::Package(pkg)
    }

    pub fn resolve_export(&self, fi: usize, name: &str, depth: usize) -> Res {
        if depth > 12 {
            return Res::Missing("re-export chain too deep".into());
        }
        let f = &self.files[fi];
        if let Some(local) = f.export_alias.get(name) {
            if let Some(&si) = f.by_name.get(local) {
                return Res::Slice((fi, si));
            }
            if let Some(imp) = f.imports.iter().find(|i| &i.local == local) {
                return self.resolve_import(fi, imp, depth + 1);
            }
        }
        if let Some(&si) = f.by_name.get(name) {
            if f.slices[si].exported {
                return Res::Slice((fi, si));
            }
        }
        for re in &f.reexports {
            let pick = match &re.names {
                Some(list) => match list.iter().find(|(_, exp)| exp == name) {
                    Some((imp, _)) => imp.clone(),
                    None => continue,
                },
                None => name.to_string(),
            };
            match self.resolve_module(fi, &re.source) {
                Module::Repo(t) => match self.resolve_export(t, &pick, depth + 1) {
                    Res::Missing(_) if re.names.is_none() => continue,
                    r => return r,
                },
                Module::Package(p) => {
                    if re.names.is_some() {
                        return Res::External(p);
                    }
                }
                Module::Missing => {}
            }
        }
        Res::Missing(format!("{} has no export named {}", f.path, name))
    }

    fn resolve_import(&self, fi: usize, imp: &parse::Import, depth: usize) -> Res {
        match self.resolve_module(fi, &imp.source) {
            Module::Repo(t) => match &imp.name {
                ImportName::Named(n) => self.resolve_export(t, n, depth),
                ImportName::Default => self.resolve_export(t, "default", depth),
                ImportName::Namespace => Res::Namespace(t),
            },
            Module::Package(p) => Res::External(p),
            Module::Missing => Res::Missing(format!("cannot find the file {}", imp.source)),
        }
    }

    pub fn resolve_ident(&self, fi: usize, name: &str, locals: &BTreeSet<String>) -> Res {
        if locals.contains(name) {
            return Res::Local;
        }
        let f = &self.files[fi];
        if let Some(&si) = f.by_name.get(name) {
            return Res::Slice((fi, si));
        }
        if !f.grammar {
            return self.from_imports(fi, name).unwrap_or_else(|| self.by_name_in_lang(fi, name));
        }
        if let Some(imp) = f.imports.iter().find(|i| i.local == name) {
            return self.resolve_import(fi, imp, 0);
        }
        if BUILTINS.contains(&name) || GLOBAL_OBJECTS.contains(&name) {
            return Res::Builtin;
        }
        Res::Missing(format!("nothing named {name} is declared or imported"))
    }

    /// Files a module path points at, by matching the end of their paths. One rule for `./x`,
    /// `../x`, `@/lib/x`, `lib/x`, `a/b` (from a.b, a::b) and `github.com/me/app/x`.
    pub fn module_files(&self, fi: usize, spec: &str) -> Vec<usize> {
        let f = &self.files[fi];
        let strip = |p: &str| -> String {
            // Only the final extension goes: Chat.client.tsx is the module Chat.client.
            let file = p.rsplit('/').next().unwrap_or(p);
            let stem = file.rsplit_once('.').map(|(a, _)| a).unwrap_or(file);
            let p = match p.rsplit_once('/') {
                Some((d, _)) => format!("{d}/{stem}"),
                None => stem.to_string(),
            };
            for tail in ["/index", "/__init__", "/mod", "/main"] {
                if let Some(x) = p.strip_suffix(tail) {
                    return x.to_string();
                }
            }
            p
        };
        let same = |g: &&usize| self.files[**g].lang == f.lang;
        if spec.starts_with('.') {
            let want = normalize(&format!("{}/{}", dirname(&f.path), spec));
            let hits: Vec<usize> = self.by_stem.get(want.rsplit('/').next().unwrap_or(&want)).into_iter().flatten().filter(same).copied().filter(|&g| strip(&self.files[g].path) == want || dirname(&self.files[g].path) == want).collect();
            return hits;
        }
        let spec = spec.trim_start_matches("@/").trim_start_matches("~/").trim_start_matches('/');
        let segs: Vec<&str> = spec.split('/').filter(|s| !s.is_empty()).collect();
        let Some(last) = segs.last() else { return vec![] };
        let cand: Vec<usize> = self.by_stem.get(*last).into_iter().flatten().filter(same).copied().collect();
        // Longest match of the path's end wins; one segment must be the whole spec or a repo-local path.
        let mut best: Vec<usize> = vec![];
        let mut best_len = 0;
        for &g in &cand {
            for hay in [strip(&self.files[g].path), dirname(&self.files[g].path).to_string()] {
                let h: Vec<&str> = hay.split('/').collect();
                let n = segs.iter().rev().zip(h.iter().rev()).take_while(|(a, b)| a == b).count();
                if n == 0 {
                    continue;
                }
                if n > best_len {
                    best_len = n;
                    best.clear();
                }
                if n == best_len && !best.contains(&g) {
                    best.push(g);
                }
            }
        }
        let enough = best_len == segs.len() || best_len >= 2 || segs[0].contains('.');
        if enough {
            best
        } else {
            vec![]
        }
    }

    fn find_in(&self, files: &[usize], name: &str, depth: usize) -> Option<SRef> {
        for &g in files {
            let f = &self.files[g];
            let local = f.export_alias.get(name).map(String::as_str).unwrap_or(name);
            if let Some(&si) = f.by_name.get(local) {
                return Some((g, si));
            }
            if depth < 6 {
                for re in &f.reexports {
                    if let Some(r) = self.find_in(&self.module_files(g, &re.source), name, depth + 1) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }

    fn package_of(spec: &str) -> String {
        let spec = spec.trim_start_matches("node:");
        let mut parts = spec.split('/');
        let first = parts.next().unwrap_or(spec);
        if first.starts_with('@') {
            format!("{}/{}", first, parts.next().unwrap_or(""))
        } else {
            first.to_string()
        }
    }

    /// Without a grammar: what a file imported under this name, read from its import lines.
    fn from_imports(&self, fi: usize, name: &str) -> Option<Res> {
        let f = &self.files[fi];
        if let Some(imp) = f.imports.iter().find(|i| i.local == name) {
            let files = self.module_files(fi, &imp.source);
            return Some(match &imp.name {
                ImportName::Namespace if !files.is_empty() => Res::Namespace(files[0]),
                ImportName::Namespace => Res::External(Self::package_of(&imp.source)),
                ImportName::Default | ImportName::Named(_) if files.is_empty() => {
                    // `use a::b` where b is itself a file.
                    let wanted = if let ImportName::Named(n) = &imp.name { n.as_str() } else { name };
                    let sub = self.module_files(fi, &format!("{}/{}", imp.source, wanted));
                    match sub.first() {
                        Some(&g) => Res::Namespace(g),
                        None => Res::External(Self::package_of(&imp.source)),
                    }
                }
                ImportName::Default => match self.find_in(&files, "default", 0) {
                    Some(r) => Res::Slice(r),
                    None => self.find_in(&files, name, 0).map(Res::Slice).unwrap_or(Res::NotFollowed),
                },
                ImportName::Named(n) => match self.find_in(&files, n, 0) {
                    Some(r) => Res::Slice(r),
                    None => {
                        let sub = self.module_files(fi, &format!("{}/{}", imp.source, n));
                        sub.first().map(|&g| Res::Namespace(g)).unwrap_or(Res::NotFollowed)
                    }
                },
            });
        }
        // `from x import *`, #include, require: names of those files are in scope.
        for imp in f.imports.iter().filter(|i| i.local == "*") {
            if let Some(r) = self.find_in(&self.module_files(fi, &imp.source), name, 0) {
                return Some(Res::Slice(r));
            }
        }
        None
    }

    /// Last resort without a grammar: no import explains the name. Prefer the same folder (Go and
    /// Java packages, Python siblings); otherwise the one definition in the language, marked as a
    /// guess because nothing but the name connects them.
    fn by_name_in_lang(&self, fi: usize, name: &str) -> Res {
        let f = &self.files[fi];
        let Some(c) = self.names.get(&(f.lang.clone(), name.to_string())) else { return Res::NotFollowed };
        // Test files are not part of what a journey runs, unless the caller is a test too.
        let caller_test = crate::output::is_test_file(&f.path);
        let c: Vec<SRef> = c.iter().copied().filter(|r| r.0 != fi && (caller_test || !crate::output::is_test_file(&self.files[r.0].path))).collect();
        let dir = dirname(&f.path);
        let near: Vec<SRef> = c.iter().copied().filter(|r| dirname(&self.files[r.0].path) == dir).collect();
        if near.len() == 1 {
            return Res::Slice(near[0]);
        }
        if c.len() == 1 && near.is_empty() {
            return Res::Guess(c[0]);
        }
        Res::NotFollowed
    }

    /// The state key for `root` or `root.key` when it names module-level data in the repo:
    /// `S.plans` gives "lib/state.ts#S.plans", and `schema.host` (a namespace) gives "db/schema.ts#host".
    pub fn data_slice(&self, fi: usize, root: &str, key: Option<&str>, locals: &BTreeSet<String>) -> Option<String> {
        if locals.contains(root) {
            return None;
        }
        match self.resolve_ident(fi, root, locals) {
            Res::Slice(r) if self.slice(r).role == Role::Data => Some(match key {
                Some(k) => format!("{}.{}", self.sid(r), k),
                None => self.sid(r),
            }),
            Res::Namespace(t) => match self.resolve_export(t, key?, 0) {
                Res::Slice(r) if self.slice(r).role == Role::Data => Some(self.sid(r)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Which package backs a value: `export const db = drizzle(client)` is drizzle-orm, and so is a
    /// small helper that returns one (`function getStripe() { return new Stripe(key) }`).
    fn backing(&self, r: SRef) -> Option<String> {
        let s = self.slice(r);
        let small = s.role == Role::Data || s.lines.len() <= 10;
        s.calls.iter().filter(|c| c.kind == CallKind::New || (small && c.kind == CallKind::Call)).find_map(|c| match &c.callee {
            Callee::Ident(n) | Callee::Member(n, _) => match self.resolve_ident(r.0, n, &s.locals) {
                Res::External(p) => Some(p),
                _ => None,
            },
            _ => None,
        })
    }

    /// Follow a method call from the name its chain starts with. A value made with `new X()`, or
    /// annotated `: X`, where X is a class in the repo, leads to X. A value that comes from a package
    /// (or from a module-level value a package made, like a database client) is that package.
    /// Literals and globals are built in. Anything else is honestly unknown.
    /// `direct` is true for `x.m()` and false when the method sits further down a chain (`x.a.m()`).
    pub fn resolve_on(&self, fi: usize, root: &str, s: &parse::Slice, depth: usize, direct: bool) -> Res {
        if !self.files[fi].grammar {
            return Res::NotFollowed;
        }
        if root == "this" {
            return Res::Local;
        }
        if root == "#literal" {
            return Res::Builtin;
        }
        if depth > 6 {
            return Res::NotFollowed;
        }
        if s.locals.contains(root) {
            return match s.inits.get(root) {
                Some(parse::Init::Literal) => Res::Builtin,
                Some(parse::Init::New(c)) | Some(parse::Init::Typed(c)) => match self.resolve_ident(fi, c, &BTreeSet::new()) {
                    Res::Slice(r) if self.slice(r).role.runs() => Res::Slice(r),
                    Res::Slice(_) => Res::NotFollowed,
                    Res::Missing(_) => Res::NotFollowed,
                    other => other,
                },
                Some(parse::Init::Call(f)) => match self.resolve_ident(fi, f, &s.locals) {
                    Res::External(p) => Res::External(p),
                    Res::Builtin => Res::Builtin,
                    Res::Slice(r) => self.backing(r).map(Res::External).unwrap_or(Res::NotFollowed),
                    _ => Res::NotFollowed,
                },
                Some(parse::Init::From(y)) if y != root => self.resolve_on(fi, y, s, depth + 1, false),
                _ => Res::NotFollowed,
            };
        }
        match self.resolve_ident(fi, root, &BTreeSet::new()) {
            Res::Slice(r) => match self.slice(r).role {
                Role::Data => match self.backing(r) {
                    Some(p) => Res::External(p),
                    // `api.save()` on an object of functions is a step; `S.sid.replace()` is a
                    // method on a value stored inside it, not a step of its own.
                    None if direct => Res::Slice(r),
                    None => Res::NotFollowed,
                },
                _ => Res::Slice(r),
            },
            Res::Namespace(_) => Res::NotFollowed,
            other => other,
        }
    }

    /// A bare name inside a method. In Java, C#, Ruby and the like it may be a sibling method; in
    /// Python, JavaScript, Rust, Go and Lua methods need `self` or `this`, so it never is one.
    fn bare_name(&self, fi: usize, name: &str, s: &parse::Slice) -> Res {
        let f = &self.files[fi];
        if s.locals.contains(name) {
            return Res::Local;
        }
        if let Some((class, _)) = s.name.rsplit_once('.') {
            if !matches!(f.lang.as_str(), "py" | "ts" | "rs" | "go" | "lua") {
                if let Some(&si) = f.by_name.get(&format!("{class}.{name}")) {
                    return Res::Slice((fi, si));
                }
            }
        }
        match f.by_name.get(name) {
            Some(&si) if f.slices[si].name != name => self.from_imports(fi, name).unwrap_or_else(|| self.by_name_in_lang(fi, name)),
            _ => self.resolve_ident(fi, name, &s.locals),
        }
    }

    /// A method on a module-level value: the package that made it, or the value itself when the repo
    /// built it (`const logger = createLogger()` leads into createLogger).
    fn on_value(&self, r: SRef) -> Res {
        if self.slice(r).role.runs() {
            return Res::Slice(r);
        }
        match self.backing(r) {
            Some(p) => Res::External(p),
            None if self.slice(r).calls.iter().any(|c| c.kind == CallKind::Call || c.kind == CallKind::New) => Res::Slice(r),
            None => Res::NotFollowed,
        }
    }

    /// A module may be one file, or a folder of them (Go packages).
    fn module_siblings(&self, t: usize) -> Vec<usize> {
        let f = &self.files[t];
        if f.lang == "go" {
            let d = dirname(&f.path);
            return (0..self.files.len()).filter(|&g| self.files[g].lang == "go" && dirname(&self.files[g].path) == d).collect();
        }
        vec![t]
    }

    /// `module.name()` without a grammar: the module is a file or folder of the same language.
    pub fn member_in_lang(&self, fi: usize, module: &str, name: &str) -> Res {
        let f = &self.files[fi];
        if matches!(module, "self" | "this" | "cls" | "Self") {
            return f.by_name.get(name).map(|&si| Res::Slice((fi, si))).unwrap_or(Res::NotFollowed);
        }
        if let Some(&si) = f.by_name.get(module) {
            return self.on_value((fi, si));
        }
        match self.from_imports(fi, module) {
            Some(Res::Namespace(t)) => {
                let folder = self.module_siblings(t);
                return self.find_in(&folder, name, 0).map(Res::Slice).unwrap_or(Res::NotFollowed);
            }
            Some(Res::Slice(r)) => return self.on_value(r),
            Some(Res::External(p)) => return Res::External(p),
            _ => {}
        }
        if let Some(files) = self.modules.get(&(f.lang.clone(), module.to_string())) {
            let hits: Vec<SRef> = files.iter().filter_map(|&g| self.files[g].by_name.get(name).map(|&si| (g, si))).collect();
            if hits.len() == 1 {
                return Res::Slice(hits[0]);
            }
        }
        match self.by_name_in_lang(fi, module) {
            Res::Slice(r) => Res::Slice(r),
            _ => Res::NotFollowed,
        }
    }

    fn resolve_member(&self, fi: usize, obj: &str, prop: &str, locals: &BTreeSet<String>) -> Res {
        match self.resolve_ident(fi, obj, locals) {
            Res::Namespace(t) => self.resolve_export(t, prop, 0),
            Res::Slice(r) => Res::Slice(r),
            Res::External(p) => Res::External(p),
            Res::Local => Res::NotFollowed,
            r => r,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Call,
    Render,
    Passed,
    Event(String),
    /// `x[name]()` may run this entry of table x.
    Table(String),
    /// Connected only by matching a name.
    Guess,
}

impl EdgeKind {
    pub fn label(&self) -> String {
        match self {
            EdgeKind::Call => "call".into(),
            EdgeKind::Render => "render".into(),
            EdgeKind::Passed => "passed as a value".into(),
            EdgeKind::Event(e) => format!("event {e}"),
            EdgeKind::Table(t) => format!("may run from {t}"),
            EdgeKind::Guess => "linked by name only".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub to: SRef,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone)]
pub struct Unres {
    pub line: usize,
    pub code: String,
    pub why: String,
}

#[derive(Default)]
pub struct Graph {
    pub edges: HashMap<SRef, Vec<Edge>>,
    pub unresolved: HashMap<SRef, Vec<Unres>>,
    pub external: HashMap<SRef, BTreeSet<String>>,
    pub not_followed: HashMap<SRef, usize>,
    pub reads: HashMap<SRef, BTreeSet<String>>,
    pub writes: HashMap<SRef, BTreeSet<String>>,
    /// event name -> handlers, from the event rule.
    pub events: BTreeMap<String, Vec<SRef>>,
    /// Calls on values, by what they reached: [followed into the repo, a package, built in, unknown].
    pub method_calls: [usize; 4],
    /// Links made only by matching a name, with no import to confirm them.
    pub guessed_links: usize,
    /// Slices that run while files load (called from top-level statements), plus what they call.
    pub on_load: BTreeSet<SRef>,
}

impl Graph {
    pub fn build(repo: &Repo) -> Graph {
        let mut g = Graph::default();
        let empty = BTreeSet::new();
        let class_methods = repo.class_methods();
        let named = |r: &Res, n: &str| matches!(r, Res::Slice(s) if repo.slice(*s).name == n);

        // Event registry first: registrations usually run on load, outside any function.
        for rule in &repo.cfg.rules.event {
            for (fi, f) in repo.files.iter().enumerate() {
                let all = f.load_calls.iter().map(|c| (c, &empty)).chain(f.slices.iter().flat_map(|s| s.calls.iter().map(move |c| (c, &s.locals))));
                for (c, locals) in all {
                    let Callee::Ident(n) = &c.callee else { continue };
                    if c.kind != CallKind::Call || !named(&repo.resolve_ident(fi, n, locals), &rule.register) {
                        continue;
                    }
                    if let (Some(Arg::Str(evt)), Some(Arg::Ident(h))) = (c.args.first(), c.args.get(1)) {
                        if let Res::Slice(hr) = repo.resolve_ident(fi, h, locals) {
                            let v = g.events.entry(evt.clone()).or_default();
                            if !v.contains(&hr) {
                                v.push(hr);
                            }
                        }
                    }
                }
            }
        }

        // `x[name]()` calls on tables, settled once every write in the repo is known.
        let mut tables: Vec<(SRef, SRef, usize, String)> = vec![];
        for (fi, f) in repo.files.iter().enumerate() {
            for (si, s) in f.slices.iter().enumerate() {
                if si == SUPPORT {
                    continue;
                }
                let me = (fi, si);
                let mut edges: Vec<Edge> = vec![];
                let add = |to: SRef, kind: EdgeKind, edges: &mut Vec<Edge>| {
                    if to != me && !edges.iter().any(|e| e.to == to) {
                        edges.push(Edge { to, kind });
                    }
                };
                for c in &s.calls {
                    let res = match &c.callee {
                        Callee::Ident(n) if !f.grammar => repo.bare_name(fi, n, s),
                        Callee::Ident(n) => repo.resolve_ident(fi, n, &s.locals),
                        Callee::Member(o, p) if !f.grammar => repo.member_in_lang(fi, o, p),
                        Callee::Member(o, p) => match repo.resolve_ident(fi, o, &BTreeSet::new()) {
                            Res::Namespace(t) if !s.locals.contains(o) => repo.resolve_export(t, p, 0),
                            _ => repo.resolve_on(fi, o, s, 0, true),
                        },
                        Callee::Computed(Some(n)) => match repo.resolve_ident(fi, n, &s.locals) {
                            Res::Slice(r) if repo.slice(r).table.is_some() => {
                                tables.push((me, r, c.line, c.text.clone()));
                                Res::Local
                            }
                            _ => Res::Missing("which function runs is looked up by name at runtime".into()),
                        },
                        Callee::Computed(None) => Res::Missing("which function runs is looked up by name at runtime".into()),
                        Callee::Other => Res::Missing("called through an expression the lifter cannot follow".into()),
                        Callee::Method(Some(root), _) => repo.resolve_on(fi, root, s, 0, false),
                        Callee::Method(None, _) => Res::NotFollowed,
                        Callee::Returned(inner) => match &**inner {
                            Callee::Ident(n) => match repo.resolve_ident(fi, n, &s.locals) {
                                Res::External(p) => Res::External(p),
                                Res::Missing(w) => Res::Missing(w),
                                _ => Res::NotFollowed,
                            },
                            _ => Res::NotFollowed,
                        },
                    };
                    let method = match &c.callee {
                        Callee::Member(_, p) | Callee::Method(_, p) => Some(p.as_str()),
                        _ => None,
                    };
                    if let Some(m) = method.filter(|m| WRITE_METHODS.contains(m)) {
                        let target = match c.args.first() {
                            Some(Arg::Ident(n)) => repo.data_slice(fi, n, None, &s.locals),
                            Some(Arg::Member(o, p)) => repo.data_slice(fi, o, Some(p), &s.locals),
                            _ => None,
                        };
                        if let Some(key) = target {
                            let _ = m;
                            g.writes.entry(me).or_default().insert(key);
                        }
                    }
                    if c.kind == CallKind::Ref {
                        // A guess needs the strongest position: a plain function argument. A mention
                        // anywhere else must be explained by the file itself or an import.
                        let name = if let Callee::Ident(n) = &c.callee { n.as_str() } else { "" };
                        let explained = f.grammar || f.by_name.contains_key(name) || f.imports.iter().any(|i| i.local == name);
                        let res = match res {
                            Res::Guess(r) if !c.args.is_empty() && repo.slice(r).role == Role::Function => Res::Slice(r),
                            Res::Slice(r) if !explained && (c.args.is_empty() || repo.slice(r).role != Role::Function) => Res::NotFollowed,
                            r => r,
                        };
                        if let Res::Slice(r) = res {
                            if repo.slice(r).role.runs() {
                                add(r, EdgeKind::Passed, &mut edges);
                            }
                        }
                        continue;
                    }
                    let method_name = match &c.callee {
                        Callee::Member(_, p) | Callee::Method(_, p) => Some(p.as_str()),
                        _ => None,
                    };
                    // this.save() is a sibling method; cart.total() is the method, not just the class.
                    let this = matches!(&c.callee, Callee::Method(Some(r), _) | Callee::Member(r, _) if matches!(r.as_str(), "this" | "self" | "cls" | "Self"));
                    let res = match (method_name, res) {
                        (Some(m), _) if this => match s.name.rsplit_once('.').and_then(|(cl, _)| f.by_name.get(&format!("{cl}.{m}"))) {
                            Some(&si2) => Res::Slice((fi, si2)),
                            None => Res::Local,
                        },
                        (Some(m), Res::Slice(r)) if repo.slice(r).role == Role::Class => {
                            let fr = &repo.files[r.0];
                            match fr.by_name.get(&format!("{}.{m}", repo.slice(r).name)) {
                                Some(&si2) => Res::Slice((r.0, si2)),
                                None => Res::Slice(r),
                            }
                        }
                        (_, r) => r,
                    };
                    let res = match res {
                        Res::NotFollowed if method_name.map(|m| BUILTIN_METHODS.contains(&m) && !class_methods.contains(m)).unwrap_or(false) => Res::Builtin,
                        r => r,
                    };
                    if matches!(c.callee, Callee::Member(..) | Callee::Method(..)) && c.kind == CallKind::Call {
                        let k = match &res {
                            Res::Slice(_) => 0,
                            Res::External(_) => 1,
                            Res::Builtin | Res::Local => 2,
                            _ => 3,
                        };
                        g.method_calls[k] += 1;
                    }
                    let (res, guessed) = match res {
                        Res::Guess(r) => (Res::Slice(r), true),
                        r => (r, false),
                    };
                    match res {
                        Res::Slice(r) => {
                            let kind = if guessed {
                                g.guessed_links += 1;
                                EdgeKind::Guess
                            } else if c.kind == CallKind::Jsx {
                                EdgeKind::Render
                            } else {
                                EdgeKind::Call
                            };
                            add(r, kind.clone(), &mut edges);
                            // Building a class runs its constructor.
                            if repo.slice(r).role == Role::Class {
                                for ctor in ["constructor", "__init__", "initialize", "new"] {
                                    if let Some(&ci) = repo.files[r.0].by_name.get(&format!("{}.{ctor}", repo.slice(r).name)) {
                                        add((r.0, ci), kind.clone(), &mut edges);
                                    }
                                }
                            }
                            for rule in &repo.cfg.rules.event {
                                if repo.slice(r).name == rule.emit {
                                    match c.args.first() {
                                        Some(Arg::Str(evt)) => {
                                            for h in g.events.get(evt).cloned().unwrap_or_default() {
                                                add(h, EdgeKind::Event(evt.clone()), &mut edges);
                                            }
                                        }
                                        _ => g.unresolved.entry(me).or_default().push(Unres {
                                            line: c.line,
                                            code: c.text.clone(),
                                            why: "event name is not a fixed string, so its handlers are unknown".into(),
                                        }),
                                    }
                                }
                            }
                        }
                        Res::External(p) => {
                            g.external.entry(me).or_default().insert(p);
                        }
                        Res::Namespace(t) => g.unresolved.entry(me).or_default().push(Unres {
                            line: c.line,
                            code: c.text.clone(),
                            why: format!("calls the whole module {}", repo.files[t].path),
                        }),
                        Res::Missing(why) => g.unresolved.entry(me).or_default().push(Unres { line: c.line, code: c.text.clone(), why }),
                        Res::NotFollowed => *g.not_followed.entry(me).or_default() += 1,
                        Res::Builtin | Res::Local | Res::Guess(_) => {}
                    }
                }
                g.edges.insert(me, edges);

                for a in &s.access {
                    let Some(key) = repo.data_slice(fi, &a.root, a.key.as_deref(), &s.locals) else { continue };
                    if key.starts_with(&format!("{}.", repo.sid(me))) || key == repo.sid(me) {
                        continue;
                    }
                    let set = if a.write { g.writes.entry(me) } else { g.reads.entry(me) };
                    set.or_default().insert(key);
                }
            }
        }

        // A table written in the code can only run what it holds, so the table and every function
        // it names join the journey. A table that can change keeps its warning.
        for (me, table, line, code) in tables {
            let t = repo.slice(table).table.as_ref().expect("only tables are queued");
            let (sid, name) = (repo.sid(table), repo.slice(table).name.clone());
            let inside = format!("{sid}.");
            let rewritten = g.writes.iter().any(|(w, keys)| *w != table && keys.iter().any(|k| *k == sid || k.starts_with(&inside)));
            let mut to = vec![table];
            to.extend(t.entries.iter().filter_map(|e| match repo.resolve_ident(table.0, e, &empty) {
                Res::Slice(r) => Some(r),
                _ => None,
            }));
            let edges = g.edges.entry(me).or_default();
            for r in to {
                if r != me && !edges.iter().any(|e| e.to == r) {
                    edges.push(Edge { to: r, kind: EdgeKind::Table(name.clone()) });
                }
            }
            if t.open || rewritten {
                g.unresolved.entry(me).or_default().push(Unres { line, code, why: format!("{name} can change while the app runs, so this may run more than its listed entries") });
            }
        }

        let mut stack: Vec<SRef> = vec![];
        for (fi, f) in repo.files.iter().enumerate() {
            for c in &f.load_calls {
                let res = match &c.callee {
                    Callee::Ident(n) => repo.resolve_ident(fi, n, &empty),
                    Callee::Member(o, p) => repo.resolve_member(fi, o, p, &empty),
                    _ => continue,
                };
                if let Res::Slice(r) = res {
                    stack.push(r);
                }
            }
        }
        // A top-level const runs its initializer on load: `createdAt: createdAt()` in a table definition.
        for (fi, f) in repo.files.iter().enumerate() {
            for (si, sl) in f.slices.iter().enumerate() {
                if sl.role == Role::Data {
                    stack.extend(g.edges.get(&(fi, si)).map(|es| es.iter().map(|e| e.to).collect::<Vec<_>>()).unwrap_or_default());
                }
            }
        }
        while let Some(r) = stack.pop() {
            if g.on_load.insert(r) {
                stack.extend(g.edges.get(&r).map(|es| es.iter().map(|e| e.to).collect::<Vec<_>>()).unwrap_or_default());
            }
        }
        g
    }
}
