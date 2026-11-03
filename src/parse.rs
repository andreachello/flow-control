//! Parse one file into slices with a one-to-one line ledger.
//!
//! Every line ends up owned by exactly one slice, or listed as unplaced. A top-level
//! declaration (function, class, const, type) is a slice, together with the comments and
//! blank lines directly above it. Imports, directives and loose comments belong to the
//! file's support slice. A statement that runs when the file loads (a top-level call, an
//! `if`, anything outside a declaration) is unplaced: the parser will not guess its owner.

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    Ident(String),
    Member(String, String),
    /// A method on some value (`a.b.c()`, `this.x()`, `foo().bar()`): the name the chain starts
    /// from, if it starts from a plain name (`this` counts), and the method called.
    Method(Option<String>, String),
    /// `x[name]()`: which function runs depends on data. Holds `x` when it is a plain name, because
    /// a table written out in the code can still be read.
    Computed(Option<String>),
    /// Calls whatever another call returned: `plugin(opts)(config)`.
    Returned(Box<Callee>),
    /// Called through some other expression.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Str(String),
    Ident(String),
    /// `schema.users`: one property on a name.
    Member(String, String),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Call,
    New,
    Jsx,
    /// A function passed as a value (`arr.map(fmt)`, `onClick={save}`). It may run later.
    Ref,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub callee: Callee,
    pub kind: CallKind,
    pub line: usize,
    pub text: String,
    pub args: Vec<Arg>,
}

/// A read or write of a name, optionally one property deep: `S.plans` gives root S, key plans.
#[derive(Debug, Clone)]
pub struct Access {
    pub root: String,
    pub key: Option<String>,
    pub write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Function,
    Class,
    Data,
    Type,
    /// describe/it/test blocks: checks written as code, the seed of runnable scenarios.
    Test,
    Support,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Function => "function",
            Role::Class => "class",
            Role::Data => "data",
            Role::Type => "type",
            Role::Test => "test",
            Role::Support => "support",
        }
    }
    pub fn runs(self) -> bool {
        matches!(self, Role::Function | Role::Class)
    }
}

/// A `const` object written out in the code, which `x[name]()` can only pick from.
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// Functions named as values: `{ a: run, b }`. Inline functions already belong to the table's slice.
    pub entries: Vec<String>,
    /// Entries may exist that the code does not list: a spread, a computed key, or a `let`.
    pub open: bool,
}

#[derive(Debug, Clone)]
pub struct Slice {
    pub table: Option<Table>,
    pub name: String,
    pub names: Vec<String>,
    pub role: Role,
    pub lines: Vec<usize>,
    pub exported: bool,
    pub calls: Vec<Call>,
    pub access: Vec<Access>,
    pub locals: BTreeSet<String>,
    /// Lines placed here by a person through flow.toml.
    pub hand: Vec<usize>,
    /// What each local name was made from, so a method called on it can be followed.
    pub inits: BTreeMap<String, Init>,
    /// Method names a class defines, so `x.push()` is not mistaken for a built-in when a class has push.
    pub methods: Vec<String>,
    /// Fingerprint of the code's tokens, ignoring spacing, line breaks and comments. Two versions
    /// with the same shape do the same thing; only their formatting differs.
    pub shape: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Init {
    /// `new X()`
    New(String),
    /// `f()` or `await f()`
    Call(String),
    /// `a.b()` or a chain starting from `a`
    From(String),
    /// A parameter or variable annotated `: X`
    Typed(String),
    /// An array, string, number, object or function written in place.
    Literal,
}

impl Slice {
    pub fn new_pub(name: &str, role: Role) -> Self {
        Slice::new(name, role)
    }
    fn new(name: &str, role: Role) -> Self {
        Slice {
            table: None,
            name: name.to_string(),
            names: vec![name.to_string()],
            role,
            lines: vec![],
            exported: false,
            calls: vec![],
            access: vec![],
            locals: BTreeSet::new(),
            hand: vec![],
            inits: BTreeMap::new(),
            methods: vec![],
            shape: String::new(),
        }
    }
    pub fn text(&self, f: &ParsedFile) -> String {
        self.lines.iter().map(|&r| f.lines[r].as_str()).collect::<Vec<_>>().join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct Unplaced {
    pub lines: Vec<usize>,
    pub first_line: usize,
    pub text: String,
    pub why: String,
}

#[derive(Debug, Clone)]
pub enum ImportName {
    Named(String),
    Default,
    Namespace,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub local: String,
    pub name: ImportName,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ReExport {
    pub source: String,
    /// None means `export * from`. Otherwise (imported, exported) pairs.
    pub names: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Slice(usize),
    Unplaced(usize),
}

#[derive(Clone)]
pub struct ParsedFile {
    /// The language family ("ts", "py", "go", ... or the bare extension for a language flow has
    /// never heard of). Names link only within one family.
    pub lang: String,
    /// True when a real grammar read this file; false for the universal reader.
    pub grammar: bool,
    /// Lines that start inside a string or comment, so nothing on them is code.
    pub quoted: BTreeSet<usize>,
    pub path: String,
    pub src: String,
    pub lines: Vec<String>,
    pub owner: Vec<Owner>,
    pub slices: Vec<Slice>,
    pub by_name: HashMap<String, usize>,
    pub unplaced: Vec<Unplaced>,
    pub imports: Vec<Import>,
    pub reexports: Vec<ReExport>,
    /// exported name -> local name (`export { a as b }`, `export default x`).
    pub export_alias: BTreeMap<String, String>,
    pub directives: Vec<String>,
    pub parse_error: bool,
    /// Lines where tree-sitter found syntax it could not read (ERROR or MISSING nodes).
    pub error_lines: Vec<usize>,
    /// Calls made by statements that run on load (unplaced or hand-placed). Only rules read these.
    pub load_calls: Vec<Call>,
}

pub const SUPPORT: usize = 0;

pub struct Ledger {
    pub total: usize,
    pub placed: usize,
    pub by_hand: usize,
    pub unplaced_lines: usize,
    pub unplaced_statements: usize,
    pub complete: bool,
    pub round_trip: bool,
}

impl ParsedFile {
    pub fn empty(path: &str, src: &str, lines: Vec<String>, lang: &str) -> ParsedFile {
        ParsedFile {
            lang: lang.to_string(),
            grammar: false,
            quoted: BTreeSet::new(),
            path: path.to_string(),
            src: src.to_string(),
            lines,
            owner: vec![],
            slices: vec![Slice::new("(support)", Role::Support)],
            by_name: HashMap::new(),
            unplaced: vec![],
            imports: vec![],
            reexports: vec![],
            export_alias: BTreeMap::new(),
            directives: vec![],
            parse_error: false,
            error_lines: vec![],
            load_calls: vec![],
        }
    }

    pub fn ledger(&self) -> Ledger {
        let placed: usize = self.slices.iter().map(|s| s.lines.len()).sum();
        let by_hand: usize = self.slices.iter().map(|s| s.hand.len()).sum();
        let unplaced_lines: usize = self.unplaced.iter().map(|u| u.lines.len()).sum();
        Ledger {
            total: self.lines.len(),
            placed,
            by_hand,
            unplaced_lines,
            unplaced_statements: self.unplaced.len(),
            complete: placed + unplaced_lines == self.lines.len(),
            round_trip: self.print() == Some(self.src.clone()),
        }
    }

    /// Rebuild the file from its owners. None if any line has zero or two owners.
    pub fn print(&self) -> Option<String> {
        let mut out: Vec<Option<&str>> = vec![None; self.lines.len()];
        let all = self
            .slices
            .iter()
            .flat_map(|s| s.lines.iter())
            .chain(self.unplaced.iter().flat_map(|u| u.lines.iter()));
        for &r in all {
            if out[r].is_some() {
                return None;
            }
            out[r] = Some(&self.lines[r]);
        }
        let v: Option<Vec<&str>> = out.into_iter().collect();
        v.map(|v| v.join("\n"))
    }
}

/// Read `const x = { ... }` into a table. Anything else is not a table.
fn table_of(n: Node, b: &[u8]) -> Option<Table> {
    let decl = if n.kind() == "export_statement" { n.child_by_field_name("declaration")? } else { n };
    if decl.kind() != "lexical_declaration" {
        return None;
    }
    let mut v = named_children(decl).into_iter().find(|d| d.kind() == "variable_declarator")?.child_by_field_name("value")?;
    while matches!(v.kind(), "as_expression" | "satisfies_expression" | "parenthesized_expression") {
        v = *named_children(v).first()?;
    }
    if v.kind() != "object" {
        return None;
    }
    let mut t = Table { entries: vec![], open: decl.child(0).map(|c| c.kind() != "const").unwrap_or(true) };
    for e in named_children(v) {
        match e.kind() {
            "pair" => {
                if e.child_by_field_name("key").map(|k| k.kind() == "computed_property_name").unwrap_or(true) {
                    t.open = true;
                }
                if let Some(x) = e.child_by_field_name("value").filter(|x| x.kind() == "identifier") {
                    t.entries.push(text(x, b));
                }
            }
            "shorthand_property_identifier" => t.entries.push(text(e, b)),
            "method_definition" | "comment" => {}
            _ => t.open = true,
        }
    }
    Some(t)
}

fn text(n: Node, b: &[u8]) -> String {
    n.utf8_text(b).unwrap_or("").to_string()
}

fn string_val(n: Node, b: &[u8]) -> String {
    let t = text(n, b);
    if t.len() >= 2 {
        t[1..t.len() - 1].to_string()
    } else {
        t
    }
}

fn has_child_kind(n: Node, kind: &str) -> bool {
    (0..n.child_count()).any(|i| n.child(i).map(|c| c.kind() == kind).unwrap_or(false))
}

fn find_child<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    (0..n.child_count()).filter_map(|i| n.child(i)).find(|c| c.kind() == kind)
}

fn named_children(n: Node) -> Vec<Node> {
    (0..n.named_child_count()).filter_map(|i| n.named_child(i)).collect()
}

fn error_rows(root: Node) -> Vec<usize> {
    let mut out = vec![];
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            out.push(n.start_position().row);
            continue;
        }
        if n.has_error() {
            stack.extend(named_children(n));
            stack.extend((0..n.child_count()).filter_map(|i| n.child(i)).filter(|c| !c.is_named() && c.is_missing()));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The name a member chain starts from: `db` in `db.select().from(x)`, `this` in `this.save()`.
fn chain_root(mut n: Node, b: &[u8]) -> Option<String> {
    loop {
        match n.kind() {
            "identifier" => return Some(text(n, b)),
            "this" => return Some("this".into()),
            "array" | "string" | "template_string" | "number" | "object" | "regex" | "arrow_function" | "function_expression" => return Some("#literal".into()),
            "member_expression" | "subscript_expression" => n = n.child_by_field_name("object")?,
            "call_expression" => n = n.child_by_field_name("function")?,
            "new_expression" => n = n.child_by_field_name("constructor")?,
            "await_expression" | "parenthesized_expression" | "non_null_expression" | "as_expression" | "satisfies_expression" => n = n.named_child(0)?,
            _ => return None,
        }
    }
}

/// Every token of a node in order, skipping comments, with whitespace inside JSX text collapsed.
fn shape_tokens(n: Node, b: &[u8], out: &mut String) {
    shape_tokens_skip(n, b, out, &HashSet::new());
}

fn shape_tokens_skip(n: Node, b: &[u8], out: &mut String, skip: &HashSet<usize>) {
    if skip.contains(&n.id()) {
        return;
    }
    match n.kind() {
        "comment" => {}
        "jsx_text" => {
            let t = text(n, b);
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            if !t.is_empty() {
                out.push_str(&t);
                out.push('\u{1}');
            }
        }
        _ if n.child_count() == 0 => {
            out.push_str(n.utf8_text(b).unwrap_or(""));
            out.push('\u{1}');
        }
        _ => {
            for i in 0..n.child_count() {
                if let Some(c) = n.child(i) {
                    shape_tokens_skip(c, b, out, skip);
                }
            }
        }
    }
}

fn init_of(v: Node, b: &[u8]) -> Init {
    match v.kind() {
        "new_expression" => match v.child_by_field_name("constructor") {
            Some(c) if c.kind() == "identifier" => Init::New(text(c, b)),
            _ => Init::Literal,
        },
        "await_expression" | "parenthesized_expression" | "non_null_expression" | "as_expression" | "satisfies_expression" => {
            v.named_child(0).map(|x| init_of(x, b)).unwrap_or(Init::Literal)
        }
        "call_expression" => match v.child_by_field_name("function") {
            Some(f) if f.kind() == "identifier" => Init::Call(text(f, b)),
            Some(f) => chain_root(f, b).map(Init::From).unwrap_or(Init::Literal),
            None => Init::Literal,
        },
        "member_expression" | "subscript_expression" => chain_root(v, b).map(Init::From).unwrap_or(Init::Literal),
        "identifier" => Init::From(text(v, b)),
        "array" | "string" | "template_string" | "number" | "object" | "arrow_function" | "function_expression" | "true" | "false" | "regex" | "binary_expression" | "unary_expression" => Init::Literal,
        _ => Init::Literal,
    }
}

fn type_name(ann: Node, b: &[u8]) -> Option<String> {
    let mut stack = vec![ann];
    while let Some(n) = stack.pop() {
        if n.kind() == "type_identifier" {
            return Some(text(n, b));
        }
        if n.kind() == "predefined_type" || n.kind() == "literal_type" {
            return None;
        }
        for i in (0..n.named_child_count()).rev() {
            if let Some(c) = n.named_child(i) {
                stack.push(c);
            }
        }
    }
    None
}

fn rows(n: Node) -> std::ops::RangeInclusive<usize> {
    let s = n.start_position().row;
    let e = n.end_position();
    let er = if e.column == 0 && e.row > s { e.row - 1 } else { e.row };
    s..=er
}

fn attachable(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

enum Item<'t> {
    Decl { names: Vec<String>, role: Role, exported: bool, node: Node<'t>, body: Node<'t> },
    Support(Node<'t>),
    Unplaced(Node<'t>, &'static str),
}

const DECL_KINDS: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_signature",
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
    "lexical_declaration",
    "variable_declaration",
    "internal_module",
    "module",
];

fn bind_names(n: Node, b: &[u8], out: &mut Vec<String>) {
    match n.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(text(n, b)),
        "type_annotation" => {}
        _ => {
            for c in named_children(n) {
                bind_names(c, b, out);
            }
        }
    }
}

const FN_WRAPPERS: &[&str] = &["memo", "forwardRef", "cache", "unstable_cache", "observer", "lazy"];

fn is_fn_value(v: Node, b: &[u8]) -> bool {
    match v.kind() {
        "arrow_function" | "function_expression" | "function" | "generator_function" => true,
        // memo(() => ...), cache(async () => ...), React.forwardRef(...). Only known wrappers:
        // z.preprocess(() => ...) also takes a callback, but it is a schema, not a function.
        "call_expression" => {
            let leaf = v.child_by_field_name("function").and_then(|f| match f.kind() {
                "identifier" => Some(f),
                "member_expression" => f.child_by_field_name("property"),
                _ => None,
            });
            leaf.map(|l| FN_WRAPPERS.contains(&text(l, b).as_str())).unwrap_or(false)
                && v.child_by_field_name("arguments").map(|a| named_children(a).iter().any(|x| is_fn_value(*x, b))).unwrap_or(false)
        }
        "parenthesized_expression" | "as_expression" | "satisfies_expression" => {
            named_children(v).first().map(|x| is_fn_value(*x, b)).unwrap_or(false)
        }
        _ => false,
    }
}

fn decl_names(n: Node, b: &[u8]) -> (Vec<String>, Role) {
    let row = n.start_position().row + 1;
    let name_of = |n: Node| {
        n.child_by_field_name("name")
            .map(|x| text(x, b).trim_matches(|c| c == '"' || c == '\'').to_string())
            .unwrap_or_else(|| format!("anonymous@{row}"))
    };
    match n.kind() {
        "function_declaration" | "generator_function_declaration" | "function_signature" => (vec![name_of(n)], Role::Function),
        "class_declaration" | "abstract_class_declaration" => (vec![name_of(n)], Role::Class),
        "interface_declaration" | "type_alias_declaration" => (vec![name_of(n)], Role::Type),
        "enum_declaration" | "internal_module" | "module" => (vec![name_of(n)], Role::Data),
        "lexical_declaration" | "variable_declaration" => {
            let mut names = vec![];
            let mut role = Role::Data;
            for d in named_children(n).into_iter().filter(|d| d.kind() == "variable_declarator") {
                if let Some(nm) = d.child_by_field_name("name") {
                    bind_names(nm, b, &mut names);
                }
                if let Some(v) = d.child_by_field_name("value") {
                    if is_fn_value(v, b) {
                        role = Role::Function;
                    } else if v.kind() == "class" && role == Role::Data {
                        role = Role::Class;
                    }
                }
            }
            if names.is_empty() {
                names.push(format!("anonymous@{row}"));
            }
            (names, role)
        }
        "ambient_declaration" => (vec![], Role::Support),
        k => (vec![format!("{k}@{row}")], Role::Data),
    }
}

fn specifiers(clause: Node, b: &[u8]) -> Vec<(String, String)> {
    named_children(clause)
        .into_iter()
        .filter(|s| s.kind() == "export_specifier")
        .filter_map(|s| {
            let name = text(s.child_by_field_name("name")?, b);
            let alias = s.child_by_field_name("alias").map(|a| text(a, b)).unwrap_or_else(|| name.clone());
            Some((name, alias))
        })
        .collect()
}

fn read_import(n: Node, b: &[u8], pf: &mut ParsedFile) {
    let Some(src) = n.child_by_field_name("source") else { return };
    let source = string_val(src, b);
    let Some(clause) = find_child(n, "import_clause") else { return };
    for c in named_children(clause) {
        match c.kind() {
            "identifier" => pf.imports.push(Import { local: text(c, b), name: ImportName::Default, source: source.clone() }),
            "namespace_import" => {
                if let Some(id) = named_children(c).into_iter().find(|x| x.kind() == "identifier") {
                    pf.imports.push(Import { local: text(id, b), name: ImportName::Namespace, source: source.clone() });
                }
            }
            "named_imports" => {
                for s in named_children(c).into_iter().filter(|s| s.kind() == "import_specifier") {
                    let Some(nm) = s.child_by_field_name("name") else { continue };
                    let name = text(nm, b);
                    let local = s.child_by_field_name("alias").map(|a| text(a, b)).unwrap_or_else(|| name.clone());
                    pf.imports.push(Import { local, name: ImportName::Named(name), source: source.clone() });
                }
            }
            _ => {}
        }
    }
}

const TEST_FNS: &[&str] = &["describe", "it", "test", "suite", "bench", "beforeAll", "beforeEach", "afterAll", "afterEach"];

/// `describe("isStayFree", ...)` becomes a test slice named `describe "isStayFree"`.
fn test_name(call: Node, b: &[u8]) -> Option<String> {
    let mut f = call.child_by_field_name("function")?;
    // describe.each([...])("x", fn), test.skip("x", fn), describe.concurrent(...)
    loop {
        match f.kind() {
            "member_expression" => f = f.child_by_field_name("object")?,
            "call_expression" => f = f.child_by_field_name("function")?,
            _ => break,
        }
    }
    let root = text(f, b);
    if f.kind() != "identifier" || !TEST_FNS.contains(&root.as_str()) {
        return None;
    }
    let title = call
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0))
        .filter(|x| x.kind() == "string" || x.kind() == "template_string")
        .map(|x| string_val(x, b));
    Some(match title {
        Some(t) => format!("{root} {t:?}"),
        None => format!("{root}@{}", call.start_position().row + 1),
    })
}

fn classify<'t>(n: Node<'t>, b: &[u8], pf: &mut ParsedFile) -> Item<'t> {
    match n.kind() {
        "import_statement" => {
            read_import(n, b, pf);
            Item::Support(n)
        }
        "export_statement" => {
            let is_default = has_child_kind(n, "default");
            if let Some(decl) = n.child_by_field_name("declaration") {
                let (names, role) = decl_names(decl, b);
                if role == Role::Support {
                    return Item::Support(n);
                }
                if is_default {
                    pf.export_alias.insert("default".into(), names[0].clone());
                }
                Item::Decl { names, role, exported: true, node: n, body: decl }
            } else if let Some(src) = n.child_by_field_name("source") {
                let source = string_val(src, b);
                match find_child(n, "export_clause") {
                    Some(c) => pf.reexports.push(ReExport { source, names: Some(specifiers(c, b)) }),
                    None if find_child(n, "namespace_export").is_none() => pf.reexports.push(ReExport { source, names: None }),
                    None => {}
                }
                Item::Support(n)
            } else if let Some(v) = n.child_by_field_name("value") {
                if v.kind() == "identifier" {
                    pf.export_alias.insert("default".into(), text(v, b));
                    return Item::Support(n);
                }
                let name = v.child_by_field_name("name").map(|x| text(x, b)).unwrap_or_else(|| "default".into());
                pf.export_alias.insert("default".into(), name.clone());
                let role = if v.kind() == "class" {
                    Role::Class
                } else if is_fn_value(v, b) {
                    Role::Function
                } else {
                    Role::Data
                };
                Item::Decl { names: vec![name], role, exported: true, node: n, body: v }
            } else {
                if let Some(c) = find_child(n, "export_clause") {
                    for (local, exported) in specifiers(c, b) {
                        pf.export_alias.insert(exported, local);
                    }
                }
                Item::Support(n)
            }
        }
        "expression_statement" => {
            let inner = n.named_child(0);
            match inner {
                Some(i) if i.kind() == "string" => {
                    pf.directives.push(string_val(i, b));
                    Item::Support(n)
                }
                // CommonJS exports: `module.exports = ...` is the file's export, not a side effect.
                Some(i) if i.kind() == "assignment_expression" => {
                    let left = i.child_by_field_name("left").map(|l| text(l, b)).unwrap_or_default();
                    if left == "module.exports" || left.starts_with("exports.") {
                        Item::Decl { names: vec![left], role: Role::Data, exported: true, node: n, body: i }
                    } else {
                        Item::Unplaced(n, "runs as soon as the file loads, outside any function")
                    }
                }
                Some(i) if i.kind() == "call_expression" => match test_name(i, b) {
                    Some(name) => Item::Decl { names: vec![name], role: Role::Test, exported: false, node: n, body: i },
                    None => Item::Unplaced(n, "runs as soon as the file loads, outside any function"),
                },
                _ => Item::Unplaced(n, "runs as soon as the file loads, outside any function"),
            }
        }
        "empty_statement" | "hash_bang_line" | "ambient_declaration" | "import_alias" => Item::Support(n),
        "ERROR" => Item::Unplaced(n, "could not be parsed"),
        k if DECL_KINDS.contains(&k) => {
            let (names, role) = decl_names(n, b);
            if role == Role::Support {
                Item::Support(n)
            } else {
                Item::Decl { names, role, exported: false, node: n, body: n }
            }
        }
        _ => Item::Unplaced(n, "a top-level statement outside any function"),
    }
}

struct Walker<'b> {
    b: &'b [u8],
    calls: Vec<Call>,
    access: Vec<Access>,
    locals: BTreeSet<String>,
    inits: BTreeMap<String, Init>,
    methods: Vec<String>,
    /// Nodes that are parts of their own, not of this walk.
    skip: HashSet<usize>,
}

impl<'b> Walker<'b> {
    fn new(b: &'b [u8]) -> Self {
        Walker { b, calls: vec![], access: vec![], locals: BTreeSet::new(), inits: BTreeMap::new(), methods: vec![], skip: HashSet::new() }
    }

    fn t(&self, n: Node) -> String {
        text(n, self.b)
    }

    fn push(&mut self, callee: Callee, kind: CallKind, at: Node, args: Vec<Arg>) {
        let text = crate::util::short(&self.t(at), 80);
        self.calls.push(Call { callee, kind, line: at.start_position().row, text, args });
    }

    fn callee(&self, f: Node) -> Option<Callee> {
        match f.kind() {
            "identifier" => Some(Callee::Ident(self.t(f))),
            "member_expression" => {
                let o = f.child_by_field_name("object")?;
                let p = f.child_by_field_name("property")?;
                if o.kind() == "identifier" {
                    Some(Callee::Member(self.t(o), self.t(p)))
                } else {
                    Some(Callee::Method(chain_root(o, self.b), self.t(p)))
                }
            }
            "subscript_expression" => Some(Callee::Computed(f.child_by_field_name("object").filter(|o| o.kind() == "identifier").map(|o| self.t(o)))),
            "import" | "super" => None,
            // An IIFE: its body is walked in place, so there is nothing further to follow.
            "arrow_function" | "function_expression" | "function" => None,
            "call_expression" => f.child_by_field_name("function").and_then(|g| self.callee(g)).map(|c| Callee::Returned(Box::new(c))),
            "parenthesized_expression" | "non_null_expression" => named_children(f).first().and_then(|x| self.callee(*x)),
            _ => Some(Callee::Other),
        }
    }

    fn args(&self, a: Option<Node>) -> Vec<Arg> {
        let Some(a) = a else { return vec![] };
        named_children(a)
            .into_iter()
            .take(2)
            .map(|x| match x.kind() {
                "string" => Arg::Str(string_val(x, self.b)),
                "template_string" if !has_child_kind(x, "template_substitution") => Arg::Str(string_val(x, self.b)),
                "identifier" => Arg::Ident(self.t(x)),
                "member_expression" => match (x.child_by_field_name("object"), x.child_by_field_name("property")) {
                    (Some(o), Some(p)) if o.kind() == "identifier" => Arg::Member(self.t(o), self.t(p)),
                    _ => Arg::Other,
                },
                _ => Arg::Other,
            })
            .collect()
    }

    fn write(&mut self, l: Node) {
        let mut n = l;
        loop {
            match n.kind() {
                "member_expression" | "subscript_expression" => {
                    let Some(o) = n.child_by_field_name("object") else { return };
                    if o.kind() == "identifier" {
                        let key = if n.kind() == "member_expression" { n.child_by_field_name("property").map(|p| self.t(p)) } else { None };
                        self.access.push(Access { root: self.t(o), key, write: true });
                        return;
                    }
                    n = o;
                }
                "identifier" => {
                    self.access.push(Access { root: self.t(n), key: None, write: true });
                    return;
                }
                "parenthesized_expression" | "non_null_expression" => match named_children(n).first() {
                    Some(x) => n = *x,
                    None => return,
                },
                _ => return,
            }
        }
    }

    /// Walk in evaluation order: arguments are recorded before the call that receives them.
    fn walk(&mut self, n: Node, wt: bool) {
        if self.skip.contains(&n.id()) {
            return;
        }
        match n.kind() {
            "call_expression" => {
                let f = n.child_by_field_name("function");
                if let Some(f) = f {
                    match f.kind() {
                        "identifier" | "import" | "super" => {}
                        "member_expression" => {
                            if let Some(o) = f.child_by_field_name("object") {
                                if o.kind() == "identifier" {
                                    self.access.push(Access { root: self.t(o), key: None, write: false });
                                } else {
                                    self.walk(o, false);
                                }
                            }
                        }
                        _ => self.walk(f, false),
                    }
                }
                let a = n.child_by_field_name("arguments");
                if let Some(a) = a {
                    self.walk(a, false);
                }
                if let Some(c) = f.and_then(|f| self.callee(f)) {
                    let args = self.args(a);
                    self.push(c, CallKind::Call, n, args);
                }
            }
            "new_expression" => {
                let a = n.child_by_field_name("arguments");
                if let Some(a) = a {
                    self.walk(a, false);
                }
                if let Some(c) = n.child_by_field_name("constructor") {
                    if c.kind() == "identifier" {
                        self.push(Callee::Ident(self.t(c)), CallKind::New, n, vec![]);
                    } else {
                        self.walk(c, false);
                    }
                }
            }
            "assignment_expression" | "augmented_assignment_expression" => {
                if let Some(r) = n.child_by_field_name("right") {
                    self.walk(r, false);
                }
                if let Some(l) = n.child_by_field_name("left") {
                    self.write(l);
                    self.walk(l, n.kind() == "assignment_expression");
                }
            }
            "update_expression" => {
                if let Some(a) = n.child_by_field_name("argument") {
                    self.write(a);
                }
                for c in named_children(n) {
                    self.walk(c, false);
                }
            }
            "member_expression" => match n.child_by_field_name("object") {
                Some(o) if o.kind() == "identifier" => {
                    if !wt {
                        let key = n.child_by_field_name("property").map(|p| self.t(p));
                        self.access.push(Access { root: self.t(o), key, write: false });
                    }
                }
                Some(o) => self.walk(o, wt),
                None => {}
            },
            "subscript_expression" => {
                if let Some(o) = n.child_by_field_name("object") {
                    self.walk(o, wt);
                }
                if let Some(i) = n.child_by_field_name("index") {
                    self.walk(i, false);
                }
            }
            "identifier" | "shorthand_property_identifier" => {
                let name = self.t(n);
                if !wt {
                    self.access.push(Access { root: name.clone(), key: None, write: false });
                }
                // Any reference may hand a function on as a value: an argument, an object field,
                // a return value. Resolution keeps only the ones that name a function in the repo.
                if !wt {
                    self.push(Callee::Ident(name), CallKind::Ref, n, vec![]);
                }
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                let nm = n.child_by_field_name("name");
                if let Some(nm) = nm {
                    match nm.kind() {
                        "identifier" => {
                            let t = self.t(nm);
                            if t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                                self.push(Callee::Ident(t), CallKind::Jsx, n, vec![]);
                            }
                        }
                        "member_expression" => {
                            if let (Some(o), Some(p)) = (nm.child_by_field_name("object"), nm.child_by_field_name("property")) {
                                if o.kind() == "identifier" {
                                    self.push(Callee::Member(self.t(o), self.t(p)), CallKind::Jsx, n, vec![]);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for c in named_children(n) {
                    if Some(c) != nm {
                        self.walk(c, false);
                    }
                }
            }
            "jsx_expression" => {
                let kids = named_children(n);
                if n.parent().map(|p| p.kind() == "jsx_attribute").unwrap_or(false) {
                    if let Some(id) = kids.iter().find(|k| k.kind() == "identifier") {
                        self.push(Callee::Ident(self.t(*id)), CallKind::Ref, n, vec![]);
                    }
                }
                for c in kids {
                    self.walk(c, false);
                }
            }
            "type_annotation" | "type_arguments" | "type_parameters" | "comment" => {}
            _ => {
                for c in named_children(n) {
                    self.walk(c, wt);
                }
            }
        }
    }

    fn bind(&mut self, n: Node) {
        let mut v = vec![];
        bind_names(n, self.b, &mut v);
        self.locals.extend(v);
    }

    fn collect_locals(&mut self, n: Node) {
        match n.kind() {
            "variable_declarator" => {
                if let Some(nm) = n.child_by_field_name("name").filter(|x| x.kind() == "identifier") {
                    let init = match (n.child_by_field_name("type").and_then(|t| type_name(t, self.b)), n.child_by_field_name("value")) {
                        (Some(t), _) => Init::Typed(t),
                        (None, Some(v)) => init_of(v, self.b),
                        _ => Init::Literal,
                    };
                    self.inits.insert(self.t(nm), init);
                }
            }
            "required_parameter" | "optional_parameter" => {
                if let (Some(p), Some(t)) = (n.child_by_field_name("pattern").filter(|x| x.kind() == "identifier"), n.child_by_field_name("type").and_then(|t| type_name(t, self.b))) {
                    self.inits.insert(self.t(p), Init::Typed(t));
                }
            }
            // A callback's parameters come from whatever it was handed to: in
            // `db.transaction((tx) => ...)` tx comes from db; in `homes.map((h) => ...)` h from homes.
            "arrow_function" | "function_expression" if n.parent().map(|p| p.kind() == "arguments").unwrap_or(false) => {
                let root = n.parent().and_then(|a| a.parent()).filter(|c| c.kind() == "call_expression").and_then(|c| c.child_by_field_name("function")).and_then(|f| match f.kind() {
                    "member_expression" => f.child_by_field_name("object").and_then(|o| chain_root(o, self.b)),
                    _ => None,
                });
                if let Some(root) = root {
                    let mut params = vec![];
                    if let Some(p) = n.child_by_field_name("parameter").filter(|x| x.kind() == "identifier") {
                        params.push(self.t(p));
                    }
                    if let Some(ps) = n.child_by_field_name("parameters") {
                        for p in named_children(ps) {
                            if p.child_by_field_name("type").is_none() {
                                if let Some(id) = p.child_by_field_name("pattern").filter(|x| x.kind() == "identifier") {
                                    params.push(self.t(id));
                                }
                            }
                        }
                    }
                    for p in params {
                        if p != root {
                            self.inits.entry(p).or_insert(Init::From(root.clone()));
                        }
                    }
                }
            }
            "method_definition" => {
                if let Some(nm) = n.child_by_field_name("name") {
                    self.methods.push(self.t(nm));
                }
            }
            _ => {}
        }
        let field = match n.kind() {
            "variable_declarator" => Some("name"),
            "required_parameter" | "optional_parameter" => Some("pattern"),
            "arrow_function" | "catch_clause" => Some("parameter"),
            "function_declaration" | "function_expression" | "function" | "generator_function_declaration" | "class_declaration" | "class" => Some("name"),
            "for_in_statement" => Some("left"),
            _ => None,
        };
        if let Some(x) = field.and_then(|f| n.child_by_field_name(f)) {
            self.bind(x);
        }
        for c in named_children(n) {
            self.collect_locals(c);
        }
    }
}

/// Read any file: with a grammar when flow has one for its language, with the universal reader
/// otherwise. Every language, including ones invented after flow was written, gets one owner per
/// line, function-by-function merges and journeys linked by name.
pub fn parse_any(path: &str, src: &str, assigns: &[(String, String)]) -> Result<ParsedFile> {
    if crate::universal::looks_generated(src) {
        Ok(crate::universal::whole(path, src))
    } else if crate::universal::lang_of(path) == "ts" {
        parse_file(path, src, assigns)
    } else {
        Ok(crate::universal::parse(path, src, assigns))
    }
}

static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, ParsedFile>>> = std::sync::OnceLock::new();

/// Parse a file, reusing an earlier parse of identical content. Walking git history re-reads the
/// same files at every commit, so most snapshots cost almost nothing after the first.
pub fn parse_file(path: &str, src: &str, assigns: &[(String, String)]) -> Result<ParsedFile> {
    let key = crate::util::fnv64(&format!("{path}\0{}\0{src}", assigns.iter().map(|(a, b)| format!("{a}\u{1}{b}")).collect::<Vec<_>>().join("\u{2}")));
    let cache = CACHE.get_or_init(Default::default);
    if let Some(p) = cache.lock().unwrap().get(&key) {
        if p.src == src {
            return Ok(p.clone());
        }
    }
    let p = parse_file_uncached(path, src, assigns)?;
    let mut c = cache.lock().unwrap();
    if c.len() > 20_000 {
        c.clear();
    }
    c.insert(key, p.clone());
    Ok(p)
}

/// `assigns`: (first line of the statement, owner name) pairs for this file.
fn parse_file_uncached(path: &str, src: &str, assigns: &[(String, String)]) -> Result<ParsedFile> {
    let tsx = [".tsx", ".jsx", ".js", ".mjs", ".cjs"].iter().any(|e| path.ends_with(e));
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = if tsx {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    parser.set_language(&lang)?;
    let tree = parser.parse(src, None).ok_or_else(|| anyhow!("tree-sitter gave up on {path}"))?;
    let root = tree.root_node();
    let b = src.as_bytes();
    let lines: Vec<String> = src.split('\n').map(str::to_string).collect();
    let n = lines.len();
    let mut owner: Vec<Option<Owner>> = vec![None; n];

    let mut pf = ParsedFile {
        lang: "ts".into(),
        grammar: true,
        quoted: BTreeSet::new(),
        path: path.to_string(),
        src: src.to_string(),
        lines,
        owner: vec![],
        slices: vec![Slice::new("(support)", Role::Support)],
        by_name: HashMap::new(),
        unplaced: vec![],
        imports: vec![],
        reexports: vec![],
        export_alias: BTreeMap::new(),
        directives: vec![],
        parse_error: root.has_error(),
        error_lines: if root.has_error() { error_rows(root) } else { vec![] },
        load_calls: vec![],
    };

    let mut shapes: HashMap<usize, String> = HashMap::new();
    let items: Vec<Item> = named_children(root)
        .into_iter()
        .filter(|c| c.kind() != "comment")
        .map(|c| classify(c, b, &mut pf))
        .collect();

    // Pass 1: declarations. Same-named declarations (TS overloads, a type and a value) share one slice.
    for it in &items {
        let Item::Decl { names, role, exported, node, body } = it else { continue };
        let idx = match names.iter().find_map(|nm| pf.by_name.get(nm).copied()) {
            Some(i) => i,
            None => {
                pf.slices.push(Slice::new(&names[0], *role));
                if *role == Role::Data {
                    pf.slices.last_mut().unwrap().table = table_of(*body, b);
                }
                pf.slices.len() - 1
            }
        };
        for nm in names {
            pf.by_name.entry(nm.clone()).or_insert(idx);
            if !pf.slices[idx].names.contains(nm) {
                pf.slices[idx].names.push(nm.clone());
            }
        }
        // Each method of a class is its own part: `Cart.total`, callable as `total`.
        let mut skip: HashSet<usize> = HashSet::new();
        if *role == Role::Class {
            let class_name = names[0].clone();
            let cbody = body.child_by_field_name("body");
            for m in cbody.map(named_children).unwrap_or_default().into_iter().filter(|m| m.kind() == "method_definition") {
                let Some(nm) = m.child_by_field_name("name") else { continue };
                let short = text(nm, b);
                let full = format!("{class_name}.{short}");
                let mi = match pf.by_name.get(&full) {
                    Some(&i) => i,
                    None => {
                        let mut sl = Slice::new(&full, Role::Function);
                        sl.names.push(short.clone());
                        sl.exported = true;
                        pf.slices.push(sl);
                        pf.by_name.insert(full, pf.slices.len() - 1);
                        pf.slices.len() - 1
                    }
                };
                for r in rows(m) {
                    if owner[r].is_none() {
                        owner[r] = Some(Owner::Slice(mi));
                    }
                }
                let mut q = m.start_position().row;
                while q > 0 && owner[q - 1].is_none() && attachable(&pf.lines[q - 1]) && q > node.start_position().row + 1 {
                    q -= 1;
                    owner[q] = Some(Owner::Slice(mi));
                }
                let mut w = Walker::new(b);
                w.walk(m, false);
                w.collect_locals(m);
                shape_tokens(m, b, shapes.entry(mi).or_default());
                let sl = &mut pf.slices[mi];
                sl.calls.extend(w.calls);
                sl.access.extend(w.access);
                sl.locals.extend(w.locals);
                for (k, v) in std::mem::take(&mut w.inits) {
                    sl.inits.entry(k).or_insert(v);
                }
                skip.insert(m.id());
            }
        }
        for r in rows(*node) {
            if owner[r].is_none() {
                owner[r] = Some(Owner::Slice(idx));
            }
        }
        let mut w = Walker::new(b);
        w.skip = skip.clone();
        w.walk(*body, false);
        w.collect_locals(*body);
        shape_tokens_skip(*node, b, shapes.entry(idx).or_default(), &skip);
        let s = &mut pf.slices[idx];
        for (k, v) in std::mem::take(&mut w.inits) {
            s.inits.entry(k).or_insert(v);
        }
        s.methods.extend(std::mem::take(&mut w.methods));
        s.exported |= *exported;
        if s.role == Role::Type && *role != Role::Type {
            s.role = *role;
        }
        s.calls.extend(w.calls);
        s.access.extend(w.access);
        s.locals.extend(w.locals);
    }

    // Pass 2: support and unplaced statements.
    for it in &items {
        match it {
            Item::Support(node) => {
                shape_tokens(*node, b, shapes.entry(SUPPORT).or_default());
                for r in rows(*node) {
                    if owner[r].is_none() {
                        owner[r] = Some(Owner::Slice(SUPPORT));
                    }
                }
            }
            Item::Unplaced(node, why) => {
                let text = text(*node, b);
                let first = text.lines().next().unwrap_or("").trim().to_string();
                let mut w = Walker::new(b);
                w.walk(*node, false);
                pf.load_calls.extend(w.calls);
                let rs: Vec<usize> = rows(*node).filter(|r| owner[*r].is_none()).collect();
                let target = assigns.iter().find(|(l, _)| l.trim() == first).and_then(|(_, to)| pf.by_name.get(to).copied());
                match target {
                    Some(t) if !rs.is_empty() => {
                        for &r in &rs {
                            owner[r] = Some(Owner::Slice(t));
                        }
                        pf.slices[t].hand.extend(rs);
                        shape_tokens(*node, b, shapes.entry(t).or_default());
                    }
                    _ => {
                        let ui = pf.unplaced.len();
                        for &r in &rs {
                            owner[r] = Some(Owner::Unplaced(ui));
                        }
                        let why = if rs.is_empty() { format!("{why} (shares a line with other code)") } else { why.to_string() };
                        pf.unplaced.push(Unplaced { lines: rs, first_line: node.start_position().row, text: crate::util::short(&text, 100), why });
                    }
                }
            }
            Item::Decl { .. } => {}
        }
    }

    // Comments and blank lines directly above a declaration belong to it.
    let mut seen = vec![false; pf.slices.len()];
    let mut firsts = vec![];
    for (r, o) in owner.iter().enumerate() {
        if let Some(Owner::Slice(i)) = o {
            if *i != SUPPORT && !seen[*i] {
                seen[*i] = true;
                firsts.push((r, *i));
            }
        }
    }
    for (r, i) in firsts {
        let mut q = r;
        while q > 0 && owner[q - 1].is_none() && attachable(&pf.lines[q - 1]) {
            q -= 1;
            owner[q] = Some(Owner::Slice(i));
        }
    }

    let mut final_owner = Vec::with_capacity(n);
    for (r, o) in owner.into_iter().enumerate() {
        let o = o.unwrap_or(Owner::Slice(SUPPORT));
        if let Owner::Slice(i) = o {
            pf.slices[i].lines.push(r);
        }
        final_owner.push(o);
    }
    pf.owner = final_owner;
    for (i, s) in pf.slices.iter_mut().enumerate() {
        s.shape = crate::util::fnv64(shapes.get(&i).map(String::as_str).unwrap_or(""));
    }
    Ok(pf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_is_exact() {
        let src = "// header\nimport { on } from './bus';\n\n// sends mail\nexport function send(k: string) {\n  log(k);\n}\n\non('x', send);\nconst A = 1, B = () => 2;\n";
        let pf = parse_file("m.ts", src, &[]).unwrap();
        let l = pf.ledger();
        assert!(l.complete && l.round_trip);
        assert_eq!(l.unplaced_statements, 1);
        assert_eq!(pf.unplaced[0].text, "on('x', send);");
        let send = &pf.slices[pf.by_name["send"]];
        assert_eq!(send.lines, vec![2, 3, 4, 5, 6]);
        assert_eq!(pf.by_name["A"], pf.by_name["B"]);
        assert_eq!(pf.slices[pf.by_name["B"]].role, Role::Function);

        let pf2 = parse_file("m.ts", src, &[("on('x', send);".into(), "send".into())]).unwrap();
        let l2 = pf2.ledger();
        assert!(l2.complete && l2.round_trip);
        assert_eq!(l2.unplaced_statements, 0);
        assert_eq!(l2.by_hand, 1);
    }

    #[test]
    fn calls_in_evaluation_order() {
        let pf = parse_file("r.ts", "export function up(p: string) {\n  charge(price(p));\n  S.plans = 1;\n}\n", &[]).unwrap();
        let s = &pf.slices[pf.by_name["up"]];
        let names: Vec<String> = s.calls.iter().filter(|c| c.kind == CallKind::Call).map(|c| format!("{:?}", c.callee)).collect();
        assert_eq!(names, vec!["Ident(\"price\")", "Ident(\"charge\")"]);
        assert!(s.access.iter().any(|a| a.root == "S" && a.key.as_deref() == Some("plans") && a.write));
        assert!(!s.access.iter().any(|a| a.root == "S" && !a.write));
    }
}
