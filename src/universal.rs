//! The universal reader: structure for any text file in any language, including ones nobody has
//! written a grammar for yet.
//!
//! It relies only on what nearly every language shares. Top-level blocks start at the left margin
//! and run until the next one; brackets and indentation keep a block open; comments, decorators and
//! attributes sit directly above the block they describe; a name followed by `(` is a call. That is
//! enough for one owner per line, function-by-function merges, formatting-blind comparison and
//! name-linked journeys. Where a real grammar exists, the grammar reader is used instead.

use crate::parse::{Arg, Call, CallKind, Callee, Import, ImportName, Owner, ParsedFile, Role, ReExport, Slice, Unplaced, SUPPORT};
use crate::util::fnv64;
use std::collections::{BTreeMap, BTreeSet};

/// The family a file belongs to, from its extension. Names only link within one family.
pub fn lang_of(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let ext = file.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let fam = match ext.as_str() {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => "ts",
        "py" | "pyi" => "py",
        "rb" | "rake" => "rb",
        "go" => "go",
        "rs" => "rs",
        "java" | "kt" | "kts" | "scala" | "groovy" => "jvm",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" => "c",
        "cs" | "fs" | "vb" => "dotnet",
        "php" => "php",
        "swift" => "swift",
        "dart" => "dart",
        "ex" | "exs" => "ex",
        "erl" | "hrl" => "erl",
        "clj" | "cljs" | "cljc" | "edn" | "lisp" | "el" | "scm" | "rkt" => "lisp",
        "hs" | "lhs" => "hs",
        "ml" | "mli" => "ml",
        "lua" => "lua",
        "r" => "r",
        "jl" => "jl",
        "sql" | "psql" | "prisma" => "sql",
        "sh" | "bash" | "zsh" | "fish" => "sh",
        "md" | "mdx" | "markdown" | "txt" | "rst" | "adoc" => "prose",
        "json" | "jsonc" | "jsonl" | "yaml" | "yml" | "toml" | "ini" | "env" | "xml" | "plist" | "csv" => "data",
        "css" | "scss" | "sass" | "less" => "css",
        "html" | "htm" | "vue" | "svelte" | "astro" | "hbs" | "ejs" | "erb" => "markup",
        "" => match file {
            "Dockerfile" | "Makefile" | "Procfile" | "Justfile" | "Rakefile" | "Gemfile" => "build",
            _ => "text",
        },
        other => return other.to_string(),
    };
    fam.to_string()
}

/// Data, prose and styling: read for the ledger, but merged and compared as text, not as code.
pub fn is_plain(path: &str) -> bool {
    matches!(lang_of(path).as_str(), "data" | "prose" | "css" | "markup" | "text")
}

/// Files that are generated, huge or not text: kept by version control, not read as code.
pub fn readable(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    const LOCKS: &[&str] = &["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "Cargo.lock", "poetry.lock", "Gemfile.lock", "composer.lock", "go.sum", "bun.lockb", "flake.lock", "uv.lock", "Pipfile.lock", "mix.lock"];
    const BINARY: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "icns", "bmp", "tiff", "psd", "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "jar",
        "war", "class", "o", "a", "so", "dylib", "dll", "exe", "bin", "wasm", "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "mov", "avi", "webm",
        "wav", "ogg", "flac", "sqlite", "db", "pyc", "lockb", "map", "snap",
    ];
    let ext = file.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    !LOCKS.contains(&file) && !BINARY.contains(&ext.as_str())
}

/// Minified or machine-written: very long lines, or a header that says so.
pub fn looks_generated(src: &str) -> bool {
    let head: String = src.lines().take(5).collect::<Vec<_>>().join("\n");
    if head.contains("Code generated") || head.contains("@generated") || head.contains("DO NOT EDIT") {
        return true;
    }
    let n = src.lines().count().max(1);
    src.len() / n > 500 && src.lines().any(|l| l.len() > 2000)
}

/// A generated file is one unit: it has an owner and a fingerprint, and its lines are not steps.
pub fn whole(path: &str, src: &str) -> ParsedFile {
    let lines: Vec<String> = src.split('\n').map(str::to_string).collect();
    let mut pf = ParsedFile::empty(path, src, lines, &lang_of(path));
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let mut s = Slice::new_pub(&name, Role::Data);
    s.lines = (0..pf.lines.len()).collect();
    s.shape = fnv64(src);
    pf.by_name.insert(name, 1);
    pf.slices.push(s);
    pf.owner = vec![Owner::Slice(1); pf.lines.len()];
    pf
}

struct Syntax {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    /// Single quotes start strings (not true for Rust lifetimes, Lisp quote or prose).
    single_quote: bool,
    /// Double-quoted strings may span lines, and raw strings exist (Rust).
    multiline_quotes: bool,
}

fn syntax(lang: &str) -> Syntax {
    match lang {
        "py" | "rb" | "sh" | "r" | "jl" | "ex" | "build" | "data" | "pl" => Syntax { line: &["#"], block: None, single_quote: true, multiline_quotes: false },
        "sql" | "hs" | "lua" => Syntax { line: &["--"], block: Some(("/*", "*/")), single_quote: true, multiline_quotes: false },
        "lisp" => Syntax { line: &[";"], block: None, single_quote: false, multiline_quotes: false },
        "erl" => Syntax { line: &["%"], block: None, single_quote: true, multiline_quotes: false },
        "ml" => Syntax { line: &[], block: Some(("(*", "*)")), single_quote: false, multiline_quotes: false },
        "rs" => Syntax { line: &["//"], block: Some(("/*", "*/")), single_quote: false, multiline_quotes: true },
        "css" => Syntax { line: &[], block: Some(("/*", "*/")), single_quote: true, multiline_quotes: false },
        "markup" => Syntax { line: &[], block: Some(("<!--", "-->")), single_quote: true, multiline_quotes: false },
        "prose" | "text" => Syntax { line: &[], block: None, single_quote: false, multiline_quotes: false },
        "php" => Syntax { line: &["//", "#"], block: Some(("/*", "*/")), single_quote: true, multiline_quotes: false },
        // C-like by default: most languages, and most new ones, comment this way.
        _ => Syntax { line: &["//"], block: Some(("/*", "*/")), single_quote: true, multiline_quotes: false },
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Num(String),
    Punct(char),
}

/// Tokens of one line, carrying block-comment and multi-line string state across lines.
/// Comments are dropped; whitespace never makes a token.
struct Lexer {
    syn: Syntax,
    in_block: bool,
    in_str: Option<String>,
}

impl Lexer {
    fn lex_embedded(&self, code: Vec<String>, out: &mut Vec<Tok>) {
        for e in code {
            let mut inner = Lexer { syn: Syntax { line: &[], block: None, single_quote: self.syn.single_quote, multiline_quotes: false }, in_block: false, in_str: None };
            out.extend(inner.line(&e).0);
        }
    }

    fn line(&mut self, line: &str) -> (Vec<Tok>, bool) {
        let c: Vec<char> = line.chars().collect();
        let mut out = vec![];
        let mut i = 0;
        let mut had_code = false;
        let starts = |i: usize, s: &str| -> bool { s.chars().enumerate().all(|(k, ch)| c.get(i + k) == Some(&ch)) };
        while i < c.len() {
            if self.in_block {
                if let Some((_, close)) = self.syn.block {
                    if starts(i, close) {
                        self.in_block = false;
                        i += close.chars().count();
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            if let Some(q) = self.in_str.clone() {
                let from = i;
                while i < c.len() && !starts(i, &q) {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let chunk: String = c[from..i.min(c.len())].iter().collect();
                let code = interpolated(&chunk);
                self.lex_embedded(code, &mut out);
                if i < c.len() {
                    self.in_str = None;
                    i += q.chars().count();
                }
                continue;
            }
            let ch = c[i];
            if ch.is_whitespace() {
                i += 1;
                continue;
            }
            if self.syn.line.iter().any(|m| starts(i, m)) {
                break;
            }
            if let Some((open, _)) = self.syn.block {
                if starts(i, open) {
                    self.in_block = true;
                    i += open.chars().count();
                    continue;
                }
            }
            had_code = true;
            // Rust raw strings: r"...", r#"..."#, br#"..."#
            if self.syn.multiline_quotes && (ch == 'r' || (ch == 'b' && c.get(i + 1) == Some(&'r'))) {
                let at = if ch == 'r' { i + 1 } else { i + 2 };
                let hashes = c[at.min(c.len())..].iter().take_while(|x| **x == '#').count();
                if c.get(at + hashes) == Some(&'"') {
                    let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                    let rest: String = c[at + hashes + 1..].iter().collect();
                    match rest.find(&close) {
                        Some(j) => {
                            out.push(Tok::Str(rest[..j].to_string()));
                            i = at + hashes + 1 + rest[..j].chars().count() + close.chars().count();
                        }
                        None => {
                            self.in_str = Some(close);
                            out.push(Tok::Str(String::new()));
                            i = c.len();
                        }
                    }
                    continue;
                }
            }
            if ch == '"' || ch == '`' || (ch == '\'' && self.syn.single_quote) {
                let triple: String = std::iter::repeat_n(ch, 3).collect();
                if starts(i, &triple) {
                    let rest: String = c[i + 3..].iter().collect();
                    match rest.find(&triple) {
                        Some(j) => {
                            out.push(Tok::Str(rest[..j].to_string()));
                            i += 3 + rest[..j].chars().count() + 3;
                        }
                        None => {
                            self.in_str = Some(triple);
                            out.push(Tok::Str(String::new()));
                            i = c.len();
                        }
                    }
                    continue;
                }
                let mut j = i + 1;
                let mut s = String::new();
                while j < c.len() && c[j] != ch {
                    if c[j] == '\\' && j + 1 < c.len() {
                        s.push(c[j]);
                        j += 1;
                    }
                    s.push(c[j]);
                    j += 1;
                }
                if j >= c.len() && (ch == '`' || (ch == '"' && self.syn.multiline_quotes)) {
                    self.in_str = Some(ch.to_string());
                }
                let code = interpolated(&s);
                out.push(Tok::Str(s));
                self.lex_embedded(code, &mut out);
                i = j + 1;
                continue;
            }
            if ch.is_alphabetic() || ch == '_' || ch == '$' {
                let mut j = i;
                while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_' || c[j] == '$' || (c[j] == '-' && lang_is_lisp(&self.syn) && j > i)) {
                    j += 1;
                }
                out.push(Tok::Word(c[i..j].iter().collect()));
                i = j;
                continue;
            }
            if ch.is_ascii_digit() {
                let mut j = i;
                while j < c.len() && (c[j].is_alphanumeric() || c[j] == '.' || c[j] == '_') {
                    j += 1;
                }
                out.push(Tok::Num(c[i..j].iter().collect()));
                i = j;
                continue;
            }
            out.push(Tok::Punct(ch));
            i += 1;
        }
        (out, had_code || self.in_str.is_some())
    }
}

/// Code embedded in a string: `${x()}`, `#{x()}`.
fn interpolated(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i + 1 < c.len() {
        if (c[i] == '$' || c[i] == '#') && c[i + 1] == '{' {
            let mut d = 0;
            let mut j = i + 1;
            while j < c.len() {
                if c[j] == '{' {
                    d += 1;
                } else if c[j] == '}' {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                j += 1;
            }
            out.push(c[(i + 2).min(c.len())..j.min(c.len())].iter().collect());
            i = j;
        }
        i += 1;
    }
    out
}

fn lang_is_lisp(s: &Syntax) -> bool {
    s.line == [";"]
}

const KEYWORDS: &[&str] = &[
    "if", "else", "elif", "for", "foreach", "while", "do", "switch", "case", "match", "when", "return", "yield", "await", "async", "new", "delete",
    "typeof", "sizeof", "catch", "try", "except", "finally", "with", "and", "or", "not", "in", "is", "def", "fn", "func", "function", "fun", "class",
    "struct", "lambda", "print", "assert", "raise", "throw", "unless", "until", "loop", "select", "from", "where", "values", "insert", "update",
    "into", "join", "on", "as", "then", "end", "begin", "let", "const", "var", "val", "super", "this", "self", "import", "use", "require",
];

const CLOSERS: &[&str] = &["}", ")", "]", "end", "fi", "done", "esac", "endif", "endfor", "endwhile", "endfunction", "endmodule", "else", "elif", "elsif", "except", "finally", "catch", "rescue", "ensure", "}"];
const IMPORTS: &[&str] = &["import ", "import{", "from ", "use ", "pub use ", "using ", "require ", "require(", "require_relative ", "#include", "package ", "module ", "library(", "source ", "extern crate", "@import", "open "];

const FN_WORDS: &[&str] = &["def", "function", "func", "fn", "fun", "sub", "proc", "procedure", "method", "macro", "defn", "defun", "defmacro", "defp", "routine", "action", "task", "rule", "define", "lemma", "theorem"];
const TYPE_WORDS: &[&str] = &["class", "struct", "enum", "interface", "trait", "impl", "module", "mod", "namespace", "extension", "record", "object", "protocol", "union", "type", "typedef", "schema", "model", "message", "service", "table", "view", "trigger", "contract", "library", "component"];
const DATA_WORDS: &[&str] = &["let", "const", "var", "val", "static", "global", "set", "constant"];

/// Name and role of a block from its first lines of code.
fn name_of(toks: &[Tok], lang: &str, _row: usize) -> (String, Role) {
    let words: Vec<(usize, &str)> = toks.iter().enumerate().filter_map(|(i, t)| if let Tok::Word(w) = t { Some((i, w.as_str())) } else { None }).collect();
    let next_word = |after: usize| words.iter().find(|(i, _)| *i > after).map(|(_, w)| w.to_string());
    // A value that is a function: `const f = () => ...`, `let g = |x| ...`, `h = lambda x: ...`.
    let arrow = toks.windows(2).any(|p| matches!((&p[0], &p[1]), (Tok::Punct('='), Tok::Punct('>')) | (Tok::Punct('-'), Tok::Punct('>')))) || words.iter().any(|(_, w)| ["function", "lambda", "fn", "async"].contains(w) );
    for (i, w) in &words {
        let lw = w.to_lowercase();
        if FN_WORDS.contains(&lw.as_str()) && !(lw == "function" && toks.get(i + 1).map(|t| *t == Tok::Punct('(')).unwrap_or(false)) {
            // Go and others put a receiver before the name: func (r *Repo) Save(
            if toks.get(i + 1) == Some(&Tok::Punct('(')) {
                let close = toks.iter().skip(i + 1).position(|t| *t == Tok::Punct(')')).map(|p| p + i + 1).unwrap_or(*i);
                if let Some(n) = next_word(close) {
                    return (n, Role::Function);
                }
            }
            if let Some(n) = next_word(*i) {
                return (n, Role::Function);
            }
        }
        if TYPE_WORDS.contains(&lw.as_str()) && lang != "prose" {
            if let Some(n) = next_word(*i).filter(|n| !["if", "not", "exists", "or", "replace"].contains(&n.to_lowercase().as_str())) {
                let role = if ["table", "view", "schema", "model", "message", "record"].contains(&lw.as_str()) {
                    Role::Data
                } else if ["type", "typedef", "interface", "protocol", "union"].contains(&lw.as_str()) {
                    Role::Type
                } else {
                    Role::Class
                };
                // CREATE TABLE IF NOT EXISTS name
                let n = if ["table", "view", "trigger"].contains(&lw.as_str()) { words.iter().filter(|(j, _)| j > i).map(|(_, w)| *w).find(|w| !["if", "not", "exists", "IF", "NOT", "EXISTS"].contains(w)).unwrap_or(&n).to_string() } else { n };
                return (n, role);
            }
        }
        if DATA_WORDS.contains(&lw.as_str()) {
            if let Some(n) = next_word(*i).filter(|n| n != "mut") .or_else(|| words.iter().skip_while(|(j, _)| j <= i).nth(1).map(|(_, w)| w.to_string())) {
                return (n, if arrow { Role::Function } else { Role::Data });
            }
        }
    }
    // name( ... or Type name( ... : a function. name = ... or name: ... : data.
    for (k, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct('(') => {
                if let Some(Tok::Word(w)) = k.checked_sub(1).and_then(|p| toks.get(p)) {
                    if !KEYWORDS.contains(&w.as_str()) {
                        return (w.clone(), Role::Function);
                    }
                }
                break;
            }
            Tok::Punct('=') | Tok::Punct(':') => {
                if let Some(Tok::Word(w)) = k.checked_sub(1).and_then(|p| toks.get(p)) {
                    return (w.clone(), Role::Data);
                }
                break;
            }
            _ => {}
        }
    }
    match words.first() {
        Some((_, w)) if lang != "prose" => (w.to_string(), Role::Data),
        _ => ("block".to_string(), Role::Data),
    }
}

/// Calls in a block: `name(`, `obj.name(`, with simple first arguments, plus every other name as a
/// possible reference. Resolution keeps only the ones that name something in the repo.
/// Is this token directly an argument: `f(x)` or `f(a, x)`, not part of a bigger expression?
fn in_args(toks: &[Tok], k: usize) -> bool {
    let before = k.checked_sub(1).and_then(|p| toks.get(p));
    let after = toks.get(k + 1);
    matches!(before, Some(Tok::Punct('(')) | Some(Tok::Punct(','))) && matches!(after, Some(Tok::Punct(',')) | Some(Tok::Punct(')')))
}

fn calls_of(toks: &[Tok], row_of: &[usize]) -> Vec<Call> {
    // Arguments run before the call that receives them, so calls are ordered by where they close.
    let mut out: Vec<(usize, Call)> = vec![];
    let close_of = |open: usize| -> usize {
        let mut d = 0;
        for (i, t) in toks.iter().enumerate().skip(open) {
            match t {
                Tok::Punct('(') => d += 1,
                Tok::Punct(')') => {
                    d -= 1;
                    if d == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        toks.len()
    };
    for (k, t) in toks.iter().enumerate() {
        let Tok::Word(w) = t else { continue };
        if KEYWORDS.contains(&w.as_str()) {
            continue;
        }
        let line = row_of[k];
        // <Photo ...> in JSX and similar: a capitalised tag is a component being used.
        if k >= 1 && toks[k - 1] == Tok::Punct('<') && w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) && !matches!(k.checked_sub(2).and_then(|p| toks.get(p)), Some(Tok::Word(_)) | Some(Tok::Num(_)) | Some(Tok::Punct(')'))) {
            out.push((k, Call { callee: Callee::Ident(w.clone()), kind: CallKind::Jsx, line, text: w.clone(), args: vec![] }));
            continue;
        }
        let prev_dot = k >= 1 && toks[k - 1] == Tok::Punct('.');
        let call = toks.get(k + 1) == Some(&Tok::Punct('('));
        // class A extends B: A's journey runs what it inherits.
        if matches!(k.checked_sub(1).and_then(|p| toks.get(p)), Some(Tok::Word(p)) if p == "extends" || p == "implements") {
            out.push((k, Call { callee: Callee::Ident(w.clone()), kind: CallKind::Call, line, text: w.clone(), args: vec![] }));
            continue;
        }
        if call {
            let mut args = vec![];
            let mut j = k + 2;
            for _ in 0..2 {
                match toks.get(j) {
                    Some(Tok::Str(s)) => args.push(Arg::Str(s.clone())),
                    Some(Tok::Word(a)) if toks.get(j + 1).map(|x| matches!(x, Tok::Punct(',') | Tok::Punct(')'))).unwrap_or(false) => args.push(Arg::Ident(a.clone())),
                    _ => args.push(Arg::Other),
                }
                match toks.iter().skip(j).position(|x| matches!(x, Tok::Punct(',') | Tok::Punct(')'))) {
                    Some(p) if toks[j + p] == Tok::Punct(',') => j = j + p + 1,
                    _ => break,
                }
            }
            let callee = if prev_dot {
                match k.checked_sub(2).and_then(|p| toks.get(p)) {
                    Some(Tok::Word(o)) if !(k >= 3 && toks[k - 3] == Tok::Punct('.')) => Callee::Member(o.clone(), w.clone()),
                    _ => Callee::Method(None, w.clone()),
                }
            } else {
                Callee::Ident(w.clone())
            };
            out.push((close_of(k + 1), Call { callee, kind: CallKind::Call, line, text: w.clone(), args }));
        } else if !prev_dot {
            // Any mention can hand something on. `args` marks a plain argument: the only position
            // where a link by name alone, with no import behind it, is believable.
            let args = if in_args(toks, k) { vec![Arg::Other] } else { vec![] };
            out.push((k, Call { callee: Callee::Ident(w.clone()), kind: CallKind::Ref, line, text: w.clone(), args }));
        }
    }
    out.sort_by_key(|(k, _)| *k);
    out.into_iter().map(|(_, c)| c).collect()
}

/// Pull what an import statement brings in, in any language. The module is kept as written; the
/// repo matches it to files by the end of its path, so no language needs its own resolver.
fn read_imports(stmt: &str, pf: &mut ParsedFile) {
    use regex::Regex;
    use std::sync::OnceLock;
    struct R {
        py_from: Regex,
        py_import: Regex,
        js: Regex,
        rust: Regex,
        jvm: Regex,
        quoted: Regex,
        php: Regex,
    }
    static RE: OnceLock<R> = OnceLock::new();
    let re = RE.get_or_init(|| R {
        py_from: Regex::new(r"^from\s+(\.*[\w.]*)\s+import\s+(.+)$").unwrap(),
        py_import: Regex::new(r"^import\s+([\w.]+(?:\s+as\s+\w+)?(?:\s*,\s*[\w.]+(?:\s+as\s+\w+)?)*)\s*;?$").unwrap(),
        js: Regex::new(r#"^(?:import|export)\s+(?:type\s+)?(.*?)\s*from\s+['"]([^'"]+)['"]"#).unwrap(),
        rust: Regex::new(r"^(?:pub\s+)?use\s+([^;]+);?").unwrap(),
        jvm: Regex::new(r"^import\s+(?:static\s+)?([\w.]+?)(\.\*)?;?\s*$").unwrap(),
        quoted: Regex::new(r#"(?:(\w+)\s+)?["<]([^">]+)[">]"#).unwrap(),
        php: Regex::new(r"^use\s+([\w\\]+)(?:\s+as\s+(\w+))?;").unwrap(),
    });
    let t = stmt.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.as_str();
    let mut add = |local: &str, name: ImportName, source: &str| pf.imports.push(Import { local: local.to_string(), name, source: source.to_string() });
    let pairs = |list: &str| -> Vec<(String, String)> {
        list.trim_matches(|c: char| "(){} ".contains(c))
            .split(',')
            .filter_map(|x| {
                let x = x.trim().trim_start_matches("type ");
                if x.is_empty() || x == "*" {
                    return None;
                }
                let mut it = x.split(" as ");
                let a = it.next()?.trim();
                let b = it.next().map(str::trim).unwrap_or(a);
                Some((a.to_string(), b.to_string()))
            })
            .collect()
    };
    if let Some(c) = re.py_from.captures(t) {
        let dots = c[1].chars().take_while(|x| *x == '.').count();
        let body = c[1][dots..].replace('.', "/");
        let source = if dots == 0 { body } else { format!("./{}{}", "../".repeat(dots - 1), body) };
        if c[2].trim() == "*" {
            add("*", ImportName::Namespace, &source);
        }
        for (a, b) in pairs(&c[2]) {
            add(&b, ImportName::Named(a), &source);
        }
    } else if let Some(c) = re.js.captures(t) {
        let (clause, source) = (c[1].to_string(), c[2].to_string());
        let mut rest = clause.as_str();
        if let Some(b) = rest.find('{') {
            for (x, y) in pairs(&rest[b..]) {
                add(&y, ImportName::Named(x), &source);
            }
            rest = &rest[..b];
        }
        for part in rest.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if let Some(ns) = part.strip_prefix("* as ") {
                add(ns.trim(), ImportName::Namespace, &source);
            } else if part == "*" {
                add("*", ImportName::Namespace, &source);
            } else {
                add(part, ImportName::Default, &source);
            }
        }
    } else if let Some(c) = re.rust.captures(t).filter(|c| c[1].contains("::")) {
        let path = c[1].trim();
        let path = path.replace("crate::", "").replace("self::", "./").replace("super::", "../").replace("::", "/");
        match path.find('{') {
            Some(b) => {
                let source = path[..b].trim_end_matches('/').to_string();
                for (x, y) in pairs(&path[b..]) {
                    add(&y, ImportName::Named(x), &source);
                }
            }
            None => {
                let (source, last) = path.rsplit_once('/').unwrap_or(("", &path));
                let (name, local) = last.split_once(" as ").unwrap_or((last, last));
                add(local.trim(), ImportName::Named(name.trim().to_string()), source);
            }
        }
    } else if let Some(c) = re.php.captures(t) {
        let path = c[1].replace('\\', "/");
        let last = path.rsplit('/').next().unwrap_or("").to_string();
        let local = c.get(2).map(|m| m.as_str().to_string()).unwrap_or_else(|| last.clone());
        add(&local, ImportName::Named(last), &path);
    } else if let Some(c) = re.jvm.captures(t).filter(|c| c[1].contains('.') && t.ends_with(';') || c.get(2).is_some()) {
        let path = c[1].replace('.', "/");
        if c.get(2).is_some() {
            add("*", ImportName::Namespace, &path);
        } else {
            let last = path.rsplit('/').next().unwrap_or("").to_string();
            add(&last, ImportName::Named(last.clone()), &path);
        }
    } else if let Some(c) = re.py_import.captures(t) {
        for (a, b) in pairs(&c[1]) {
            let local = if a == b { a.split('.').next_back().unwrap_or(&a).to_string() } else { b };
            add(&local, ImportName::Namespace, &a.replace('.', "/"));
        }
    } else {
        // Go blocks, #include, require, source, and whatever a new language writes: quoted paths.
        for c in re.quoted.captures_iter(t) {
            let source = c[2].to_string();
            let stem = source.rsplit('/').next().unwrap_or("").split('.').next().unwrap_or("").to_string();
            let alias = c.get(1).map(|m| m.as_str()).filter(|a| !["import", "require", "include", "using", "source", "use", "from", "package", "module", "require_relative"].contains(a));
            match alias {
                Some(a) => add(a, ImportName::Namespace, &source),
                None => {
                    add(&stem, ImportName::Namespace, &source);
                    add("*", ImportName::Namespace, &source);
                }
            }
        }
    }
}

fn is_import(line: &str) -> bool {
    IMPORTS.iter().any(|p| line.starts_with(p)) && !line.contains(" = function") && !line.starts_with("module.exports")
}

/// A line that starts with a short run of symbols then a space reads as a comment even in a language
/// whose comment marker the reader does not know: `~~ note`, `;; note`, `%% note`, `-- note`.
fn looks_like_comment(t: &str) -> bool {
    let sym: String = t.chars().take_while(|c| !c.is_alphanumeric() && !c.is_whitespace() && !"([{\"'`<$@#_".contains(*c)).collect();
    !sym.is_empty() && sym.chars().count() <= 3 && t[sym.len()..].starts_with(' ')
}

fn is_lead(line: &str, syn: &Syntax) -> bool {
    let t = line.trim_start();
    t.is_empty()
        || (looks_like_comment(t) && !syn.line.is_empty())
        || t.starts_with('@')
        || t.starts_with("#[")
        || t.starts_with("#!")
        || syn.line.iter().any(|m| t.starts_with(m))
        || syn.block.map(|(o, _)| t.starts_with(o)).unwrap_or(false)
        || t.starts_with('*')
        || (t.starts_with('[') && t.ends_with(']') && !t.contains('=')) // C# attributes
}

pub fn parse(path: &str, src: &str, assigns: &[(String, String)]) -> ParsedFile {
    let lang = lang_of(path);
    let syn = syntax(&lang);
    let lines: Vec<String> = src.split('\n').map(str::to_string).collect();
    let n = lines.len();
    let mut lex = Lexer { syn, in_block: false, in_str: None };
    let mut toks_by_line: Vec<Vec<Tok>> = Vec::with_capacity(n);
    let mut code_line = vec![false; n];
    let mut in_comment_before = vec![false; n];
    for (r, l) in lines.iter().enumerate() {
        in_comment_before[r] = lex.in_block || lex.in_str.is_some();
        let (t, code) = lex.line(l);
        code_line[r] = code;
        toks_by_line.push(t);
    }
    let syn = syntax(&lang);

    // Where blocks start: a line of code at the left margin, outside any open bracket, that does not
    // close or continue the block before it. In prose, a heading.
    let mut depth: i64 = 0;
    let mut starts = vec![false; n];
    for r in 0..n {
        let line = &lines[r];
        let at_margin = !line.starts_with(' ') && !line.starts_with('\t');
        let t = line.trim();
        let closes = is_closer(t);
        starts[r] = if lang == "prose" {
            t.starts_with('#')
        } else {
            code_line[r] && at_margin && depth <= 0 && !closes && !in_comment_before[r] && !is_lead(line, &syn)
        };
        for tok in &toks_by_line[r] {
            match tok {
                Tok::Punct('(') | Tok::Punct('[') | Tok::Punct('{') => depth += 1,
                Tok::Punct(')') | Tok::Punct(']') | Tok::Punct('}') => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            depth = 0;
        }
    }

    // Code with no structure in its layout (assembly, unindented scripts): when nearly every line
    // would be its own block, split at blank lines, labels and definition keywords instead.
    let code_lines = code_line.iter().filter(|c| **c).count();
    let start_lines = starts.iter().filter(|c| **c).count();
    if code_lines >= 20 && start_lines * 10 >= code_lines * 6 && !matches!(lang.as_str(), "prose" | "data" | "css" | "markup" | "text") {
        for r in 0..n {
            if !starts[r] {
                continue;
            }
            let prev_gap = r == 0 || is_lead(&lines[r - 1], &syn);
            let first = lines[r].split(|c: char| !c.is_alphanumeric() && c != '_').find(|w| !w.is_empty()).unwrap_or("").to_lowercase();
            let keyword = FN_WORDS.contains(&first.as_str()) || TYPE_WORDS.contains(&first.as_str());
            let label = lines[r].trim_end().ends_with(':') && !lines[r].contains(' ');
            starts[r] = prev_gap || keyword || label;
        }
    }

    let mut pf = ParsedFile::empty(path, src, lines.clone(), &lang);
    pf.quoted = (0..n).filter(|&r| in_comment_before[r]).collect();
    let mut owner: Vec<Option<Owner>> = vec![None; n];
    let mut seen_names: BTreeMap<String, usize> = BTreeMap::new();
    let block_starts: Vec<usize> = (0..n).filter(|&r| starts[r]).collect();
    let plain = matches!(lang.as_str(), "prose" | "data" | "css" | "markup" | "text");

    for (bi, &s) in block_starts.iter().enumerate() {
        let end = block_starts.get(bi + 1).copied().unwrap_or(n);
        // Trailing blank lines and the next block's comments or decorators belong to the next block.
        let mut e = end;
        while e > s + 1 && is_lead(&lines[e - 1], &syn) {
            e -= 1;
        }
        let line0 = lines[s].trim();
        // `module X` with a body is a definition (Ruby, Verilog, Pascal); alone it is a declaration.
        if is_import(line0) && lang != "prose" && !(line0.starts_with("module ") && e > s + 1) {
            for row in owner.iter_mut().take(e).skip(s) {
                *row = Some(Owner::Slice(SUPPORT));
            }
            read_imports(&lines[s..e].join(" "), &mut pf);
            continue;
        }
        if line0.starts_with("export ") && (line0.contains(" from \"") || line0.contains(" from '")) {
            for row in owner.iter_mut().take(e).skip(s) {
                *row = Some(Owner::Slice(SUPPORT));
            }
            let t = lines[s..e].join(" ");
            read_imports(&t, &mut pf);
            let src = t.split(" from ").last().unwrap_or("").trim().trim_end_matches(';').trim_matches(|c| c == '"' || c == '\'').to_string();
            pf.reexports.push(ReExport { source: src, names: None });
            continue;
        }
        if line0.starts_with("export default") {
            let head: Vec<Tok> = (s..e).take(3).flat_map(|r| toks_by_line[r].clone()).collect();
            let (n, _) = name_of(&head, &lang, s);
            pf.export_alias.insert("default".into(), n);
        }
        let head: Vec<Tok> = (s..e).take(3).flat_map(|r| toks_by_line[r].clone()).collect();
        let (mut name, role) = if line0.starts_with("if __name__") { ("__main__".to_string(), Role::Function) } else { name_of(&head, &lang, s) };
        if seen_names.contains_key(&name) {
            // Numbered by order, never by line: adding a line above must not rename anything.
            let mut k = 2;
            while seen_names.contains_key(&format!("{name}~{k}")) {
                k += 1;
            }
            name = format!("{name}~{k}");
        }
        let idx = pf.slices.len();
        seen_names.insert(name.clone(), idx);
        let mut sl = Slice::new_pub(&name, if plain { Role::Data } else { role });
        sl.exported = true;
        pf.by_name.insert(name.clone(), idx);
        pf.slices.push(sl);
        for row in owner.iter_mut().take(e).skip(s) {
            *row = Some(Owner::Slice(idx));
        }
        // Inside a class, each method is its own part, at any depth: `Billing.Cart.total`.
        if role == Role::Class && !plain {
            let plain_name = name.split('~').next().unwrap_or(&name).to_string();
            let cx = Carve { lines: &lines, toks: &toks_by_line, code: &code_line, in_comment: &in_comment_before, syn: &syn, lang: &lang };
            cx.carve(s, e, &plain_name, idx, 0, &mut pf, &mut owner, &mut seen_names);
        }
    }
    // Comments, decorators and blank lines directly above a block belong to it; everything else
    // before the first block (headers, imports) is the file's support.
    for &s in &block_starts {
        let Some(Owner::Slice(i)) = owner[s] else { continue };
        let mut q = s;
        while q > 0 && owner[q - 1].is_none() && is_lead(&lines[q - 1], &syn) {
            q -= 1;
            owner[q] = Some(Owner::Slice(i));
        }
    }
    let mut stray: Vec<usize> = vec![];
    for (r, o) in owner.iter_mut().enumerate() {
        if o.is_none() {
            if code_line[r] && !is_import(lines[r].trim()) && !is_lead(&lines[r], &syn) && !plain {
                stray.push(r);
            } else {
                *o = Some(Owner::Slice(SUPPORT));
            }
        }
    }
    // Code outside any block (rare: only when a file starts indented) is listed, never guessed.
    for r in stray {
        let t = lines[r].trim().to_string();
        if let Some(&to) = assigns.iter().find(|(l, _)| l.trim() == t).and_then(|(_, to)| pf.by_name.get(to)) {
            owner[r] = Some(Owner::Slice(to));
            pf.slices[to].hand.push(r);
            continue;
        }
        let ui = pf.unplaced.len();
        owner[r] = Some(Owner::Unplaced(ui));
        pf.unplaced.push(Unplaced { lines: vec![r], first_line: r, text: crate::util::short(&t, 100), why: "code outside any block the reader could find".into() });
    }
    for (r, o) in owner.into_iter().enumerate() {
        let o = o.unwrap();
        if let Owner::Slice(i) = o {
            pf.slices[i].lines.push(r);
        }
        pf.owner.push(o);
    }
    // Fingerprints and calls come from each part's final lines. Decorators count for the fingerprint
    // but are not steps: they run when the code loads, not during a journey.
    for s in pf.slices.iter_mut() {
        let toks: Vec<Tok> = s.lines.iter().flat_map(|&r| toks_by_line[r].clone()).collect();
        s.shape = fnv64(&tok_text(&toks));
        if s.role != Role::Support && !plain {
            let body: Vec<usize> = s.lines.iter().copied().filter(|&r| !lines[r].trim_start().starts_with('@') && !lines[r].trim_start().starts_with("#[")).collect();
            let t: Vec<Tok> = body.iter().flat_map(|&r| toks_by_line[r].clone()).collect();
            let rows: Vec<usize> = body.iter().flat_map(|&r| std::iter::repeat_n(r, toks_by_line[r].len())).collect();
            s.calls = calls_of(&t, &rows);
            // `def total(` reads like a call to itself: a definition is not a step.
            let own = s.name.rsplit('.').next().unwrap_or(&s.name).split('~').next().unwrap_or("").to_string();
            if let Some(i) = s.calls.iter().position(|c| c.kind == CallKind::Call && c.callee == Callee::Ident(own.clone())) {
                s.calls.remove(i);
            }
        }
        s.locals = locals_of(&s.lines.iter().flat_map(|&r| toks_by_line[r].clone()).collect::<Vec<_>>());
    }
    pf
}

/// Names a block defines for itself: parameters, assignments, `let x`, `for x`. Good enough to stop
/// a local variable linking to something with the same name elsewhere in the repo.
fn locals_of(t: &[Tok]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // A bracket group is a parameter list only when something that starts a body follows it; a
    // call's arguments (`cache(async () => ...)`) are not parameters.
    for (open, x) in t.iter().enumerate() {
        if *x != Tok::Punct('(') {
            continue;
        }
        let mut d = 0;
        let mut close = None;
        for (j, y) in t.iter().enumerate().skip(open) {
            match y {
                Tok::Punct('(') => d += 1,
                Tok::Punct(')') => {
                    d -= 1;
                    if d == 0 {
                        close = Some(j);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let after = (t.get(close + 1), t.get(close + 2));
        let arrow = matches!(after, (Some(Tok::Punct('=')), Some(Tok::Punct('>'))) | (Some(Tok::Punct('-')), Some(Tok::Punct('>'))));
        let head = open == 0 || matches!(t.get(open - 1), Some(Tok::Word(_)));
        let body = matches!(after.0, Some(Tok::Punct('{')) | Some(Tok::Punct(':')));
        let keyworded = open >= 2 && matches!(t.get(open - 2), Some(Tok::Word(w)) if FN_WORDS.contains(&w.to_lowercase().as_str()));
        if arrow || (head && body) || keyworded {
            for y in &t[open..close] {
                if let Tok::Word(w) = y {
                    out.insert(w.clone());
                }
            }
        }
    }
    // x => ...
    for w in t.windows(3) {
        if let [Tok::Word(a), Tok::Punct('='), Tok::Punct('>')] = w {
            out.insert(a.clone());
        }
    }
    for (i, x) in t.iter().enumerate() {
        let Tok::Word(w) = x else { continue };
        let next = t.get(i + 1);
        let after = t.get(i + 2);
        let prev = i.checked_sub(1).and_then(|p| t.get(p));
        let assigned = next == Some(&Tok::Punct('=')) && after != Some(&Tok::Punct('='));
        let go_short = next == Some(&Tok::Punct(':')) && after == Some(&Tok::Punct('='));
        let declared = matches!(prev, Some(Tok::Word(p)) if ["let", "const", "var", "val", "mut", "for", "foreach"].contains(&p.as_str()));
        if (assigned || go_short || declared) && prev != Some(&Tok::Punct('.')) {
            out.insert(w.clone());
        }
    }
    out
}

struct Carve<'a> {
    lines: &'a [String],
    toks: &'a [Vec<Tok>],
    code: &'a [bool],
    in_comment: &'a [bool],
    syn: &'a Syntax,
    lang: &'a str,
}

impl Carve<'_> {
    /// Give each member of a class block its own part, going down through nested classes.
    /// Functions inside functions stay with their parent: they only run as part of it.
    #[allow(clippy::too_many_arguments)]
    fn carve(&self, s: usize, e: usize, prefix: &str, idx: usize, depth: usize, pf: &mut ParsedFile, owner: &mut [Option<Owner>], seen: &mut BTreeMap<String, usize>) {
        if depth > 8 {
            return;
        }
        for (ms, me) in members(self.lines, self.toks, self.code, self.in_comment, s, e, self.syn) {
            let head: Vec<Tok> = (ms..me).take(3).flat_map(|r| self.toks[r].clone()).collect();
            let (mname, mrole) = name_of(&head, self.lang, ms);
            if !matches!(mrole, Role::Function | Role::Class) {
                continue;
            }
            let full = format!("{prefix}.{mname}");
            if seen.contains_key(&full) {
                continue; // an overload stays with the first of its name
            }
            let mi = pf.slices.len();
            seen.insert(full.clone(), mi);
            let mut msl = Slice::new_pub(&full, mrole);
            msl.exported = true;
            msl.names.push(mname.clone());
            pf.by_name.insert(full.clone(), mi);
            pf.by_name.entry(mname).or_insert(mi);
            pf.slices.push(msl);
            let mut q = ms;
            while q > s + 1 && owner[q - 1] == Some(Owner::Slice(idx)) && is_lead(&self.lines[q - 1], self.syn) {
                q -= 1;
            }
            for row in owner.iter_mut().take(me).skip(q) {
                *row = Some(Owner::Slice(mi));
            }
            if mrole == Role::Class {
                self.carve(ms, me, &full, mi, depth + 1, pf, owner, seen);
            }
        }
    }
}

/// The methods inside a class block, as (first line, line after the end). The body is whatever sits
/// at the class's first level of indentation; a method runs until the next one at that level.
fn members(lines: &[String], toks: &[Vec<Tok>], code: &[bool], in_comment: &[bool], s: usize, e: usize, syn: &Syntax) -> Vec<(usize, usize)> {
    let indent = |r: usize| lines[r].len() - lines[r].trim_start().len();
    let body = (s + 1..e).filter(|&r| code[r] && !is_closer(lines[r].trim()) && !in_comment[r] && !is_lead(&lines[r], syn)).map(indent).filter(|&i| i > 0).min();
    let Some(body) = body else { return vec![] };
    let mut depth: i64 = 0;
    let mut base: Option<i64> = None;
    let mut heads = vec![];
    for r in s..e {
        if r > s && code[r] && indent(r) == body && !in_comment[r] && !is_closer(lines[r].trim()) && !is_lead(&lines[r], syn) {
            let b = *base.get_or_insert(depth);
            if depth == b {
                heads.push(r);
            }
        }
        for t in &toks[r] {
            match t {
                Tok::Punct('(') | Tok::Punct('[') | Tok::Punct('{') => depth += 1,
                Tok::Punct(')') | Tok::Punct(']') | Tok::Punct('}') => depth -= 1,
                _ => {}
            }
        }
    }
    let mut out = vec![];
    for (i, &h) in heads.iter().enumerate() {
        let mut me = heads.get(i + 1).copied().unwrap_or(e);
        // The class's closing line and anything leading into the next method stay outside.
        while me > h + 1 && (is_lead(&lines[me - 1], syn) || (indent(me - 1) < body && is_closer(lines[me - 1].trim()))) {
            me -= 1;
        }
        out.push((h, me));
    }
    out
}

fn is_closer(t: &str) -> bool {
    let first = t.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '}' && c != ')' && c != ']').next().unwrap_or("");
    CLOSERS.contains(&first) || t.starts_with('}') || t.starts_with(')') || t.starts_with(']')
}

/// The names used in a piece of code, in order.
pub fn words(path: &str, text: &str) -> Vec<String> {
    let mut lex = Lexer { syn: syntax(&lang_of(path)), in_block: false, in_str: None };
    text.split('\n').flat_map(|l| lex.line(l).0).filter_map(|t| if let Tok::Word(w) = t { Some(w) } else { None }).collect()
}

/// The values written in a piece of code, in order: text, numbers, true and false. The flag marks
/// styling, like a class list, so a restyle does not read as a change of content.
pub fn literals(path: &str, text: &str) -> Vec<(String, bool)> {
    let mut lex = Lexer { syn: syntax(&lang_of(path)), in_block: false, in_str: None };
    let toks: Vec<Tok> = text.split('\n').flat_map(|l| lex.line(l).0).collect();
    let mut out = vec![];
    for (i, t) in toks.iter().enumerate() {
        let style = i >= 2 && toks[i - 1] == Tok::Punct('=') && matches!(&toks[i - 2], Tok::Word(w) if ["className", "class", "style", "sx"].contains(&w.as_str()));
        match t {
            Tok::Str(s) if !s.trim().is_empty() => out.push((format!("\"{s}\""), style)),
            Tok::Num(n) => out.push((n.clone(), false)),
            Tok::Word(w) if w == "true" || w == "false" => out.push((w.clone(), false)),
            _ => {}
        }
    }
    out
}

fn tok_text(t: &[Tok]) -> String {
    let mut s = String::new();
    for x in t {
        match x {
            Tok::Word(w) | Tok::Num(w) => s.push_str(w),
            Tok::Str(v) => {
                s.push('"');
                s.push_str(v);
                s.push('"');
            }
            Tok::Punct(c) => s.push(*c),
        }
        s.push('\u{1}');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &ParsedFile) -> Vec<String> {
        p.slices.iter().skip(1).map(|s| s.name.clone()).collect()
    }

    #[test]
    fn python() {
        let src = "import os\nfrom db import save\n\n# Prices\n@app.get(\"/price\")\ndef price(n):\n    total = base(n)\n\n    return save(total)\n\nclass Cart:\n    def add(self):\n        pass\n\nRATE = 0.2\n";
        let p = parse("app/main.py", src, &[]);
        assert_eq!(names(&p), ["price", "Cart", "Cart.add", "RATE"]);
        let l = p.ledger();
        assert!(l.complete && l.round_trip);
        let price = &p.slices[p.by_name["price"]];
        assert_eq!(price.lines, (2..9).collect::<Vec<_>>());
        assert!(price.calls.iter().any(|c| c.callee == Callee::Ident("base".into()) && c.kind == CallKind::Call));
        // The decorator marks a route; its own call is not a step of the journey.
        assert!(!price.calls.iter().any(|c| c.callee == Callee::Member("app".into(), "get".into())));
    }

    #[test]
    fn go_and_an_invented_language() {
        let go = "package main\n\nimport \"net/http\"\n\n// Hello says hi.\nfunc (s *Server) Hello(w http.ResponseWriter) {\n\tgreet(w)\n}\n\nfunc main() {\n\thttp.HandleFunc(\"/hello\", hello)\n}\n";
        let p = parse("main.go", go, &[]);
        assert_eq!(names(&p), ["Hello", "main"]);
        assert!(p.ledger().round_trip);
        let zz = "~~ a language nobody has heard of\ndefine greet(name) [\n  say(\"hi \" ++ name)\n]\n\ndefine shout(name) [\n  greet(upper(name))\n]\n";
        let p = parse("hello.zz", zz, &[]);
        assert_eq!(names(&p), ["greet", "shout"]);
        assert!(p.ledger().round_trip);
        assert!(p.slices[p.by_name["shout"]].calls.iter().any(|c| c.callee == Callee::Ident("greet".into())));
    }

    #[test]
    fn formatting_does_not_change_shape() {
        let a = parse("x.py", "def f(a, b):\n    return a+b\n", &[]);
        let b = parse("x.py", "def f(a,  b):\n\n    # add them\n    return a + b\n", &[]);
        assert_eq!(a.slices[1].shape, b.slices[1].shape);
    }
}
