//! The promise: a merge's result depends only on the commits, as git's does. Same commits, same
//! tree, whichever way round, in any order, on any machine, with any personal git settings.

use flowcontrol::merge;
use flowcontrol::names::Names;
use flowcontrol::vcs::Vcs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn write(d: &Path, f: &str, t: &str) {
    if let Some(p) = d.join(f).parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(d.join(f), t).unwrap();
}

fn chmod_x(d: &Path, f: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(d.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn commit(d: &Path, m: &str) {
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", m]);
}

fn branch(d: &Path, name: &str, from: &str) {
    git(d, &["checkout", "-q", "-b", name, from]);
}

fn repo(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("det-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &["init", "-q", "-b", "main"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "T"]);
    d
}

fn open(d: &Path) -> Vcs {
    Vcs::open(d, "flows", Names::default()).unwrap()
}

/// Merge `theirs` into `ours` on a throwaway branch and return the tree id.
fn tree_of(d: &Path, ours: &str, theirs: &str) -> String {
    git(d, &["checkout", "-q", "--detach", ours]);
    let p = merge::plan(&mut open(d), theirs).unwrap();
    assert!(p.collisions.is_empty(), "{:?}", p.collisions);
    p.tree
}

const M: &str = "export function a() {\n  return 1;\n}\n\nexport function b() {\n  return 1;\n}\n";

#[test]
fn either_way_round_gives_the_same_tree() {
    let d = repo("swap");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "p", "main");
    let s = std::fs::read_to_string(d.join("lib/m.ts")).unwrap();
    write(&d, "lib/m.ts", &format!("import {{ p }} from \"./p\";\n{s}\nexport function p2() {{\n  return 2;\n}}\n"));
    commit(&d, "p");
    branch(&d, "q", "main");
    write(&d, "lib/m.ts", &format!("import {{ q }} from \"./q\";\n{s}\nexport function q2() {{\n  return 3;\n}}\n"));
    commit(&d, "q");
    assert_eq!(tree_of(&d, "p", "q"), tree_of(&d, "q", "p"));
}

#[test]
fn formatting_on_one_side_merges_the_same_either_way() {
    let d = repo("format");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "p", "main");
    write(&d, "lib/m.ts", &M.replace("return 1;\n}\n\nexport function b", "return (1);\n}\n\nexport function b").replace("  return 1;\n}\n\nexport function b", "    return 1;\n}\n\nexport function b"));
    commit(&d, "reformat");
    branch(&d, "q", "main");
    write(&d, "lib/m.ts", &format!("// new\n{M}"));
    commit(&d, "comment line");
    assert_eq!(tree_of(&d, "p", "q"), tree_of(&d, "q", "p"));
}

#[test]
fn a_move_merges_the_same_either_way() {
    let d = repo("moved");
    write(&d, "lib/m.ts", M);
    write(&d, "lib/n.ts", "export function n() {\n  return 0;\n}\n");
    commit(&d, "base");
    branch(&d, "mover", "main");
    write(&d, "lib/m.ts", "export function a() {\n  return 1;\n}\n");
    write(&d, "lib/n.ts", "export function n() {\n  return 0;\n}\n\nexport function b() {\n  return 1;\n}\n");
    commit(&d, "move b");
    branch(&d, "editor", "main");
    write(&d, "lib/m.ts", &M.replace("function b() {\n  return 1;", "function b() {\n  return 2;"));
    commit(&d, "edit b");
    let t = tree_of(&d, "mover", "editor");
    assert_eq!(t, tree_of(&d, "editor", "mover"));
    assert!(git(&d, &["show", &format!("{t}:lib/n.ts")]).contains("return 2;"));
    assert!(!git(&d, &["show", &format!("{t}:lib/m.ts")]).contains("function b"));
}

#[test]
fn branches_that_merged_each_other_match_git() {
    let d = repo("crisscross");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "l", "main");
    write(&d, "lib/m.ts", &M.replacen("return 1;", "return 2;", 1));
    commit(&d, "l1");
    branch(&d, "r", "main");
    let both = "export function a() {\n  return 2;\n}\n\nexport function b() {\n  return 2;\n}\n";
    write(&d, "lib/m.ts", &M.replace("function b() {\n  return 1;", "function b() {\n  return 2;"));
    commit(&d, "r1");
    let (l1, r1) = (git(&d, &["rev-parse", "l"]), git(&d, &["rev-parse", "r"]));
    git(&d, &["checkout", "-q", "l"]);
    git(&d, &["merge", "-q", "--no-edit", &r1]);
    git(&d, &["checkout", "-q", "r"]);
    git(&d, &["merge", "-q", "--no-edit", &l1]);
    assert_eq!(std::fs::read_to_string(d.join("lib/m.ts")).unwrap(), both);
    write(&d, "lib/m.ts", &both.replacen("return 2;", "return 3;", 1));
    commit(&d, "r2");
    git(&d, &["checkout", "-q", "l"]);
    write(&d, "lib/m.ts", &both.replace("function b() {\n  return 2;", "function b() {\n  return 3;"));
    commit(&d, "l2");
    assert_eq!(git(&d, &["merge-base", "--all", "l", "r"]).lines().count(), 2);
    let flow = tree_of(&d, "l", "r");
    assert_eq!(flow, tree_of(&d, "r", "l"));
    git(&d, &["checkout", "-q", "l"]);
    git(&d, &["merge", "-q", "--no-edit", "r"]);
    assert_eq!(flow, git(&d, &["rev-parse", "HEAD^{tree}"]));
}

#[test]
fn a_collision_in_a_virtual_ancestor_is_not_silently_settled() {
    let d = repo("poison");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "l", "main");
    let edit = |n: &str| M.replacen("return 1;", &format!("return {n};"), 1);
    write(&d, "lib/m.ts", &edit("2"));
    commit(&d, "l1");
    branch(&d, "r", "main");
    write(&d, "lib/m.ts", &edit("3"));
    commit(&d, "r1");
    let (l1, r1) = (git(&d, &["rev-parse", "l"]), git(&d, &["rev-parse", "r"]));
    // Each side decides the same conflict its own way, keeping its own value.
    git(&d, &["checkout", "-q", "l"]);
    git(&d, &["merge", "-q", "--no-edit", "-s", "ours", &r1]);
    git(&d, &["checkout", "-q", "r"]);
    git(&d, &["merge", "-q", "--no-edit", "-s", "ours", &l1]);
    assert_eq!(git(&d, &["merge-base", "--all", "l", "r"]).lines().count(), 2);
    git(&d, &["checkout", "-q", "l"]);
    let p = merge::plan(&mut open(&d), "r").unwrap();
    assert_eq!(p.collisions.len(), 1);
    assert_eq!(p.collisions[0].name, "a");
}

#[test]
fn binaries_modes_and_links_survive_exactly() {
    let d = repo("bytes");
    let png: &[u8] = b"\xff\xfe\x00\x01old";
    std::fs::write(d.join("logo.png"), png).unwrap();
    write(&d, "run.sh", "#!/bin/sh\n");
    write(&d, "nul.txt", "a\0b\n");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    std::fs::write(d.join("logo.png"), b"\xff\xfe\x00\x01new").unwrap();
    std::fs::write(d.join("new.bin"), b"\x00\x80\xff").unwrap();
    chmod_x(&d, "run.sh");
    std::os::unix::fs::symlink("run.sh", d.join("link.sh")).unwrap();
    commit(&d, "x");
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/z.ts", "export const z = 1;\n");
    commit(&d, "y");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    merge::apply(&v, &p, "x").unwrap();
    v.save("merge").unwrap();
    let flow = git(&d, &["rev-parse", "HEAD^{tree}"]);
    git(&d, &["checkout", "-q", "--detach", "main~1"]);
    git(&d, &["merge", "-q", "--no-edit", "x"]);
    assert_eq!(git(&d, &["rev-parse", "HEAD^{tree}"]), flow, "git's merge agrees");
    git(&d, &["checkout", "-q", "main"]);
    assert_eq!(std::fs::read(d.join("logo.png")).unwrap(), b"\xff\xfe\x00\x01new");
    assert!(git(&d, &["ls-tree", "HEAD", "link.sh"]).starts_with("120000"));
    assert!(git(&d, &["ls-tree", "HEAD", "run.sh"]).starts_with("100755"));
}

#[test]
fn one_sides_chmod_and_the_others_edit_both_stay() {
    let d = repo("chmod");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    chmod_x(&d, "lib/m.ts");
    commit(&d, "chmod");
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/m.ts", &M.replacen("return 1;", "return 2;", 1));
    commit(&d, "edit");
    let t = tree_of(&d, "main", "x");
    assert_eq!(t, tree_of(&d, "x", "main"));
    assert!(git(&d, &["ls-tree", &t, "lib/m.ts"]).starts_with("100755"));
    assert!(git(&d, &["show", &format!("{t}:lib/m.ts")]).contains("return 2;"));
}

#[test]
fn a_binary_collision_resolves_to_exact_bytes() {
    let d = repo("bincollide");
    std::fs::write(d.join("a.png"), b"\xff\x00base").unwrap();
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    std::fs::write(d.join("a.png"), b"\xff\x00theirs").unwrap();
    commit(&d, "x");
    git(&d, &["checkout", "-q", "main"]);
    std::fs::write(d.join("a.png"), b"\xff\x00ours").unwrap();
    commit(&d, "y");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    assert_eq!(p.collisions.len(), 1);
    merge::apply(&v, &p, "x").unwrap();
    merge::resolve(&v, "a.png", "theirs").unwrap();
    assert_eq!(std::fs::read(d.join("a.png")).unwrap(), b"\xff\x00theirs");
    merge::abort(&v).unwrap();
    assert_eq!(std::fs::read(d.join("a.png")).unwrap(), b"\xff\x00ours");
    assert_eq!(git(&d, &["status", "--porcelain"]), "");
}

#[test]
fn generated_flows_are_rebuilt_and_never_collide() {
    let d = repo("generated");
    write(&d, "app/api/x/route.ts", "import { a, b } from \"../../../lib/m\";\nexport async function POST() {\n  return a() + b();\n}\n");
    write(&d, "lib/m.ts", M);
    let mut v = open(&d);
    let l = flowcontrol::lift(&d, "flows").unwrap();
    flowcontrol::write(&d.join("flows"), &l.out.files).unwrap();
    v.save("base").unwrap();
    branch(&d, "x", "main");
    write(&d, "lib/m.ts", &M.replacen("return 1;", "return 2;", 1));
    open(&d).save("x").unwrap();
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/m.ts", &M.replace("function b() {\n  return 1;", "function b() {\n  return 2;"));
    open(&d).save("y").unwrap();
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    assert!(p.collisions.is_empty(), "{:?}", p.collisions);
    assert_eq!(p.tree, tree_of(&d, "x", "main"));
    git(&d, &["checkout", "-q", "main"]);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    merge::apply(&v, &p, "x").unwrap();
    assert_eq!(git(&d, &["write-tree"]), p.tree);
    v.save("merge").unwrap();
    assert_eq!(git(&d, &["rev-parse", "HEAD^{tree}"]), p.tree, "save changed the merged result");
    let fresh = flowcontrol::lift(&d, "flows").unwrap();
    assert!(flowcontrol::drift(&fresh.out.files, &flowcontrol::read_existing(&d.join("flows")).unwrap()).is_clean());
}

#[test]
fn ignored_files_do_not_change_a_lift() {
    let d = repo("ignored");
    write(&d, ".gitignore", "secret.ts\nbuild-cache/\n");
    write(&d, "app/api/x/route.ts", "export async function POST() {\n  return 1;\n}\n");
    commit(&d, "base");
    let clean = flowcontrol::lift(&d, "flows").unwrap().out.files;
    write(&d, "secret.ts", "export function hidden() {\n  return 1;\n}\n");
    write(&d, "build-cache/x.ts", "export function cached() {\n  return 1;\n}\n");
    assert_eq!(clean, flowcontrol::lift(&d, "flows").unwrap().out.files);
    // A new file that is not ignored would be saved, so it counts.
    write(&d, "lib/new.ts", "export function fresh() {\n  return 1;\n}\n");
    assert_ne!(clean, flowcontrol::lift(&d, "flows").unwrap().out.files);
}

#[test]
fn the_folder_and_the_commit_lift_to_the_same_bytes() {
    let d = repo("folder-commit");
    write(&d, "app/api/x/route.ts", "export async function POST() {\n  return 1;\n}\n");
    std::os::unix::fs::symlink("app/api/x/route.ts", d.join("alias.ts")).unwrap();
    commit(&d, "base");
    let dir = flowcontrol::lift(&d, "flows").unwrap().out.files;
    let rev = flowcontrol::lift_source(&flowcontrol::source::Source::Rev { root: d.clone(), rev: git(&d, &["rev-parse", "HEAD"]) }, "flows").unwrap().out.files;
    assert_eq!(dir, rev);
}

#[test]
fn a_mode_change_alone_can_be_saved() {
    let d = repo("modesave");
    write(&d, "run.sh", "#!/bin/sh\n");
    open(&d).save("base").unwrap();
    chmod_x(&d, "run.sh");
    open(&d).save("chmod").unwrap();
    assert!(git(&d, &["ls-tree", "HEAD", "run.sh"]).starts_with("100755"));
}

fn octopus_repo(name: &str) -> PathBuf {
    let d = repo(name);
    write(&d, "lib/m.ts", M);
    write(&d, "lib/n.ts", "export function n() {\n  return 0;\n}\n");
    commit(&d, "base");
    for (b, f, text) in [("o1", "lib/one.ts", "export const one = 1;\n"), ("o2", "lib/two.ts", "export const two = 2;\n"), ("o3", "lib/three.ts", "export const three = 3;\n")] {
        branch(&d, b, "main");
        write(&d, f, text);
        commit(&d, b);
    }
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/n.ts", "export function n() {\n  return 9;\n}\n");
    commit(&d, "main moves");
    d
}

fn revs(x: &[&str]) -> Vec<String> {
    x.iter().map(|s| s.to_string()).collect()
}

#[test]
fn many_branches_at_once_ignore_the_order_named() {
    let d = octopus_repo("octopus");
    let mut v = open(&d);
    let a = merge::plan_many(&mut v, &revs(&["o1", "o2", "o3"])).unwrap();
    let b = merge::plan_many(&mut v, &revs(&["o3", "o1", "o2"])).unwrap();
    assert_eq!(a.tree, b.tree);
    merge::apply(&v, &a, "o1, o2, o3").unwrap();
    v.save("octopus").unwrap();
    assert_eq!(git(&d, &["rev-parse", "HEAD^{tree}"]), a.tree);
    assert_eq!(v.commits(1, "HEAD").unwrap()[0].parents.len(), 4);
    git(&d, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&d, &["merge", "-q", "--no-edit", "o1", "o2", "o3"]);
    assert_eq!(git(&d, &["rev-parse", "HEAD^{tree}"]), a.tree, "git's octopus agrees");
}

#[test]
fn a_collision_stops_a_many_branch_merge_before_anything_is_written() {
    let d = octopus_repo("octopus-stop");
    git(&d, &["checkout", "-q", "o2"]);
    write(&d, "lib/n.ts", "export function n() {\n  return 5;\n}\n");
    commit(&d, "rival");
    git(&d, &["checkout", "-q", "main"]);
    let mut v = open(&d);
    let p = merge::plan_many(&mut v, &revs(&["o1", "o2"])).unwrap();
    assert!(p.blocked.is_some());
    assert!(merge::apply(&v, &p, "o1, o2").is_err());
    assert_eq!(git(&d, &["status", "--porcelain"]), "");
}

#[test]
fn merging_a_branch_already_in_or_ahead_is_plain() {
    let d = octopus_repo("octopus-small");
    let mut v = open(&d);
    let main = git(&d, &["rev-parse", "HEAD"]);
    assert!(merge::plan_many(&mut v, &revs(&["HEAD~1"])).unwrap().already_in);
    git(&d, &["checkout", "-q", "-b", "ahead"]);
    write(&d, "x.txt", "x\n");
    commit(&d, "ahead");
    git(&d, &["checkout", "-q", "main"]);
    assert!(merge::plan_many(&mut v, &revs(&["ahead", &main])).unwrap().fast_forward);
}

#[test]
fn a_merge_will_not_overwrite_a_file_git_does_not_track() {
    let d = repo("untracked");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    write(&d, "notes.txt", "theirs\n");
    commit(&d, "x");
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/z.ts", "export const z = 1;\n");
    commit(&d, "y");
    write(&d, "notes.txt", "my unsaved work\n");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    assert!(merge::apply(&v, &p, "x").is_err());
    assert_eq!(std::fs::read_to_string(d.join("notes.txt")).unwrap(), "my unsaved work\n");
    assert!(!v.merge_state_path().exists());
}

#[test]
fn merging_works_from_a_linked_worktree() {
    let d = repo("worktree");
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    write(&d, "lib/m.ts", &M.replacen("return 1;", "return 2;", 1));
    commit(&d, "x");
    let w = Path::new(env!("CARGO_TARGET_TMPDIR")).join("det-worktree-linked");
    let _ = std::fs::remove_dir_all(&w);
    git(&d, &["worktree", "prune"]);
    git(&d, &["worktree", "add", "-q", "--detach", w.to_str().unwrap(), "main"]);
    git(&w, &["checkout", "-q", "-b", "side"]);
    write(&w, "lib/z.ts", "export const z = 1;\n");
    commit(&w, "side");
    let mut v = open(&w);
    let p = merge::plan(&mut v, "x").unwrap();
    merge::apply(&v, &p, "x").unwrap();
    v.save("merge").unwrap();
    assert_eq!(v.commits(1, "HEAD").unwrap()[0].parents.len(), 2);
}

#[test]
fn personal_git_settings_do_not_change_a_text_merge() {
    let t = || merge::text_merge("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nc\nD\n").unwrap();
    assert_eq!(t().unwrap(), "a\nB\nc\nD\n");
    assert_eq!(merge::text_merge("a\nb\nc\nd\n", "a\nb\nc\nD\n", "a\nB\nc\nd\n").unwrap(), t());
}

#[test]
fn two_processes_produce_the_same_bytes() {
    // Rust seeds each process's hash maps at random, so one process proves nothing.
    let bin = env!("CARGO_BIN_EXE_flow");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo");
    let sums: Vec<String> = (0..4)
        .map(|i| {
            let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("det-proc-{i}"));
            let _ = std::fs::remove_dir_all(&out);
            let st = Command::new(bin).args(["-C", src.to_str().unwrap(), "--out", out.to_str().unwrap(), "lift"]).output().unwrap();
            assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
            let mut names: Vec<_> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().path()).collect();
            names.sort();
            names.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect::<Vec<_>>().join("\0")
        })
        .collect();
    assert!(sums.iter().all(|s| s == &sums[0]));
}

#[test]
fn windows_line_endings_do_not_change_a_lift() {
    let d = repo("crlf");
    let lf = "export async function POST() {\n  return 1;\n}\n";
    write(&d, "app/api/x/route.ts", lf);
    commit(&d, "base");
    let head = git(&d, &["rev-parse", "HEAD"]);
    let rev = flowcontrol::lift_source(&flowcontrol::source::Source::Rev { root: d.clone(), rev: head }, "flows").unwrap().out.files;
    git(&d, &["config", "core.autocrlf", "true"]);
    write(&d, "app/api/x/route.ts", &lf.replace('\n', "\r\n"));
    assert_eq!(rev, flowcontrol::lift(&d, "flows").unwrap().out.files);
    // Without conversion, CRLF is a real edit that a save would record, so it must show.
    git(&d, &["config", "core.autocrlf", "false"]);
    assert_ne!(rev, flowcontrol::lift(&d, "flows").unwrap().out.files);
}

#[test]
fn gitattributes_rules_are_honoured_from_the_commit() {
    let d = repo("attrs");
    write(&d, ".gitattributes", "changes.txt merge=union\nlocked.txt -merge\ncustom.txt merge=mydriver\n");
    for f in ["changes.txt", "locked.txt", "custom.txt"] {
        write(&d, f, "a\nb\nc\n");
    }
    write(&d, "lib/m.ts", M);
    commit(&d, "base");
    branch(&d, "x", "main");
    write(&d, "changes.txt", "a\nx\nc\n");
    write(&d, "locked.txt", "a\nb\nc\nx\n");
    write(&d, "custom.txt", "a\nb\nc\nx\n");
    commit(&d, "x");
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "changes.txt", "a\ny\nc\n");
    write(&d, "locked.txt", "y\na\nb\nc\n");
    write(&d, "custom.txt", "y\na\nb\nc\n");
    commit(&d, "y");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    let hit: Vec<&str> = p.collisions.iter().map(|c| c.file.as_str()).collect();
    assert_eq!(hit, ["locked.txt"], "union merges, -merge stops, a named driver changes nothing");
    let t = p.files["changes.txt"].clone().unwrap();
    assert!(t.contains('x') && t.contains('y'), "{t}");
    git(&d, &["checkout", "-q", "x"]);
    let q = merge::plan(&mut open(&d), "main").unwrap();
    assert_eq!(q.files["changes.txt"], p.files["changes.txt"], "union lines come out in a fixed order");
}

#[test]
fn an_edit_follows_a_renamed_file_either_way_round() {
    let d = repo("rename");
    let body = |n: u32| format!("{M}\nexport function c() {{\n  return {n};\n}}\n\nexport function e() {{\n  return 5;\n}}\n");
    write(&d, "lib/old.ts", &body(1));
    write(&d, "notes.md", "one\ntwo\nthree\nfour\nfive\n");
    commit(&d, "base");
    branch(&d, "mover", "main");
    git(&d, &["mv", "lib/old.ts", "lib/new.ts"]);
    git(&d, &["mv", "notes.md", "docs.md"]);
    commit(&d, "rename");
    branch(&d, "editor", "main");
    write(&d, "lib/old.ts", &body(2));
    write(&d, "notes.md", "one\ntwo\nthree\nfour\nFIVE\n");
    commit(&d, "edit");
    let t = tree_of(&d, "mover", "editor");
    assert_eq!(t, tree_of(&d, "editor", "mover"));
    assert!(git(&d, &["show", &format!("{t}:lib/new.ts")]).contains("return 2;"));
    assert!(git(&d, &["show", &format!("{t}:docs.md")]).contains("FIVE"));
    git(&d, &["checkout", "-q", "--detach", "mover"]);
    git(&d, &["merge", "-q", "--no-edit", "editor"]);
    assert_eq!(t, git(&d, &["rev-parse", "HEAD^{tree}"]), "git agrees");
}

#[test]
fn resolving_changes_the_named_function_even_with_a_twin() {
    let d = repo("twin");
    let src = |a: u32, b: u32| format!("export function twin() {{\n  return {a};\n}}\n\nexport function other() {{\n  return {b};\n}}\n");
    write(&d, "lib/m.ts", &src(1, 1));
    commit(&d, "base");
    branch(&d, "x", "main");
    write(&d, "lib/m.ts", &src(1, 3));
    commit(&d, "x");
    git(&d, &["checkout", "-q", "main"]);
    write(&d, "lib/m.ts", &src(2, 2));
    commit(&d, "y");
    let mut v = open(&d);
    let p = merge::plan(&mut v, "x").unwrap();
    assert_eq!(p.collisions.len(), 1);
    merge::apply(&v, &p, "x").unwrap();
    merge::resolve(&v, "other", "theirs").unwrap();
    assert_eq!(std::fs::read_to_string(d.join("lib/m.ts")).unwrap(), src(2, 3));
}

const LONG: &str = "export function a() {\n  return 1;\n}\n\nexport function b() {\n  return 1;\n}\n\nexport function c() {\n  return 1;\n}\n\nexport function d() {\n  return 1;\n}\n";

/// lib/a.ts renamed to lib/l.ts on `l` and lib/r.ts on `r`, with different functions edited.
fn two_names(name: &str) -> PathBuf {
    let d = repo(name);
    write(&d, "lib/a.ts", LONG);
    write(&d, "other.ts", "export const o = 1;\n");
    commit(&d, "base");
    branch(&d, "l", "main");
    git(&d, &["mv", "lib/a.ts", "lib/l.ts"]);
    write(&d, "lib/l.ts", &LONG.replacen("return 1;", "return 2;", 1));
    commit(&d, "l");
    branch(&d, "r", "main");
    git(&d, &["mv", "lib/a.ts", "lib/r.ts"]);
    write(&d, "lib/r.ts", &LONG.replace("function d() {\n  return 1;", "function d() {\n  return 3;"));
    commit(&d, "r");
    d
}

fn has(d: &Path, rev: &str, f: &str) -> bool {
    Command::new("git").arg("-C").arg(d).args(["cat-file", "-e", &format!("{rev}:{f}")]).status().unwrap().success()
}

#[test]
fn two_names_for_one_file_is_a_collision_and_keeps_both_edits() {
    let d = two_names("rename2");
    git(&d, &["checkout", "-q", "l"]);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "r").unwrap();
    assert_eq!(p.collisions.len(), 1);
    assert_eq!(p.collisions[0].renamed_to.as_deref(), Some("lib/r.ts"));
    assert!(p.collisions[0].label.contains("lib/l.ts") && p.collisions[0].label.contains("lib/r.ts"));
    assert!(!has(&d, &p.tree, "lib/r.ts") && !has(&d, &p.tree, "lib/a.ts"), "never in both places");
    let t = git(&d, &["show", &format!("{}:lib/l.ts", p.tree)]);
    assert!(t.contains("return 2;") && t.contains("return 3;"), "{t}");
    merge::apply(&v, &p, "r").unwrap();
    assert!(v.save("too early").is_err());
    merge::resolve(&v, "lib/l.ts", "ours").unwrap();
    v.save("keep l").unwrap();
    assert!(has(&d, "HEAD", "lib/l.ts") && !has(&d, "HEAD", "lib/r.ts"));
}

#[test]
fn the_same_choice_of_name_gives_the_same_tree_either_way_round() {
    let d = two_names("rename2-swap");
    let finish = |ours: &str, theirs: &str, side: &str| -> String {
        git(&d, &["checkout", "-q", "--detach", ours]);
        let mut v = open(&d);
        let p = merge::plan(&mut v, theirs).unwrap();
        merge::apply(&v, &p, theirs).unwrap();
        merge::resolve(&v, "different name", side).unwrap();
        v.save("merge").unwrap();
        git(&d, &["rev-parse", "HEAD^{tree}"])
    };
    let a = finish("l", "r", "theirs");
    assert_eq!(a, finish("r", "l", "ours"));
    assert!(has(&d, &a, "lib/r.ts") && !has(&d, &a, "lib/l.ts"));
}

#[test]
fn undoing_after_choosing_a_name_restores_everything() {
    let d = two_names("rename2-abort");
    git(&d, &["checkout", "-q", "l"]);
    let before = git(&d, &["rev-parse", "HEAD^{tree}"]);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "r").unwrap();
    merge::apply(&v, &p, "r").unwrap();
    merge::resolve(&v, "lib/l.ts", "theirs").unwrap();
    merge::abort(&v).unwrap();
    assert_eq!(git(&d, &["status", "--porcelain"]), "");
    assert_eq!(git(&d, &["write-tree"]), before);
    assert!(d.join("lib/l.ts").exists() && !d.join("lib/r.ts").exists());
}

#[test]
fn a_function_collision_follows_the_name_chosen() {
    let d = repo("rename2-fn");
    write(&d, "lib/a.ts", LONG);
    commit(&d, "base");
    branch(&d, "l", "main");
    git(&d, &["mv", "lib/a.ts", "lib/l.ts"]);
    write(&d, "lib/l.ts", &LONG.replacen("return 1;", "return 2;", 1));
    commit(&d, "l");
    branch(&d, "r", "main");
    git(&d, &["mv", "lib/a.ts", "lib/r.ts"]);
    write(&d, "lib/r.ts", &LONG.replacen("return 1;", "return 3;", 1));
    commit(&d, "r");
    git(&d, &["checkout", "-q", "l"]);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "r").unwrap();
    assert_eq!(p.collisions.len(), 2);
    merge::apply(&v, &p, "r").unwrap();
    merge::resolve(&v, "different name", "theirs").unwrap();
    merge::resolve(&v, "a", "theirs").unwrap();
    v.save("merge").unwrap();
    assert_eq!(git(&d, &["show", "HEAD:lib/r.ts"]) + "\n", LONG.replacen("return 1;", "return 3;", 1));
}

#[test]
fn a_binary_with_two_names_resolves_to_exact_bytes() {
    let d = repo("rename2-bin");
    let mut bytes = vec![0u8, 0xff];
    bytes.extend(std::iter::repeat(7u8).take(4000));
    let edited = |tail: &[u8]| [bytes.as_slice(), tail].concat();
    std::fs::write(d.join("a.bin"), &bytes).unwrap();
    commit(&d, "base");
    branch(&d, "l", "main");
    git(&d, &["mv", "a.bin", "l.bin"]);
    std::fs::write(d.join("l.bin"), edited(b"\xffL")).unwrap();
    commit(&d, "l");
    branch(&d, "r", "main");
    git(&d, &["mv", "a.bin", "r.bin"]);
    std::fs::write(d.join("r.bin"), edited(b"\xffR")).unwrap();
    commit(&d, "r");
    git(&d, &["checkout", "-q", "l"]);
    let mut v = open(&d);
    let p = merge::plan(&mut v, "r").unwrap();
    assert_eq!(p.collisions.len(), 1);
    assert!(p.collisions[0].take_whole);
    merge::apply(&v, &p, "r").unwrap();
    merge::resolve(&v, "l.bin", "theirs").unwrap();
    v.save("merge").unwrap();
    assert_eq!(std::fs::read(d.join("r.bin")).unwrap(), edited(b"\xffR"));
    assert!(!d.join("l.bin").exists());
}

#[test]
fn both_sides_choosing_one_new_name_merge_like_any_edit() {
    let d = repo("rename-same");
    write(&d, "lib/a.ts", LONG);
    commit(&d, "base");
    branch(&d, "l", "main");
    git(&d, &["mv", "lib/a.ts", "lib/n.ts"]);
    write(&d, "lib/n.ts", &LONG.replacen("return 1;", "return 2;", 1));
    commit(&d, "l");
    branch(&d, "r", "main");
    git(&d, &["mv", "lib/a.ts", "lib/n.ts"]);
    write(&d, "lib/n.ts", &LONG.replace("function d() {\n  return 1;", "function d() {\n  return 3;"));
    commit(&d, "r");
    let t = tree_of(&d, "l", "r");
    assert_eq!(t, tree_of(&d, "r", "l"));
    git(&d, &["checkout", "-q", "--detach", "l"]);
    git(&d, &["merge", "-q", "--no-edit", "r"]);
    assert_eq!(t, git(&d, &["rev-parse", "HEAD^{tree}"]), "git agrees");
}
