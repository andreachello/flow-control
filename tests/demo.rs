use std::path::{Path, PathBuf};

fn demo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo")
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    for e in walkdir::WalkDir::new(from) {
        let e = e.unwrap();
        let rel = e.path().strip_prefix(from).unwrap();
        if rel.starts_with("flows") {
            continue;
        }
        let dest = to.join(rel);
        if e.file_type().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
        } else {
            std::fs::copy(e.path(), &dest).unwrap();
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    copy_dir(&demo(), &d);
    d
}

fn steps(l: &flowcontrol::Lift, id: &str) -> Vec<String> {
    let f = l.flows.iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no flow {id}"));
    f.steps.iter().map(|s| l.repo.slice(s.at).name.clone()).collect()
}

#[test]
fn every_line_has_one_owner_except_the_wiring() {
    let l = flowcontrol::lift(&demo(), "flows").unwrap();
    for f in &l.repo.files {
        let led = f.ledger();
        assert!(led.complete && led.round_trip, "{} does not round trip", f.path);
    }
    let s = &l.out.stats;
    assert_eq!(s.unplaced.len(), 1);
    assert_eq!(s.unplaced[0].0, "lib/mailer.ts");
    assert_eq!(s.unplaced[0].2, "on(\"plan_changed\", onPlanChanged);");
}

#[test]
fn flows_follow_calls_namespaces_and_events_in_order() {
    let l = flowcontrol::lift(&demo(), "flows").unwrap();
    let ids: Vec<&str> = l.flows.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["GET /api/plugin", "POST /api/agent", "POST /api/cancel", "POST /api/login", "POST /api/signup", "POST /api/upgrade"]);
    assert_eq!(
        steps(&l, "POST /api/upgrade"),
        ["POST", "loadSession", "resolvePrice", "isEnabled", "chargeCard", "applyPlanChange", "emit", "onPlanChanged", "sendEmail", "renderHeader"]
    );
    assert_eq!(steps(&l, "POST /api/agent"), ["POST", "loadSession", "planGuard", "buildSystemPrompt", "isEnabled", "pickTools", "streamReply"]);
    let plugin = l.flows.iter().find(|f| f.id == "GET /api/plugin").unwrap();
    // plugins[name]() can only pick from the table written in the code, so the table is a step.
    assert!(plugin.unresolved.is_empty());
    assert_eq!(steps(&l, "GET /api/plugin"), ["GET", "runPlugin", "plugins"]);
    assert_eq!(l.out.stats.loose, ["lib/plugins.ts#legacyExport"]);
}

#[test]
fn footprints_are_read_statically() {
    let l = flowcontrol::lift(&demo(), "flows").unwrap();
    let f = &l.repo.files[l.repo.file_idx("lib/billing.ts").unwrap()];
    let charge = (l.repo.file_idx("lib/billing.ts").unwrap(), f.by_name["chargeCard"]);
    let w: Vec<&String> = l.graph.writes[&charge].iter().collect();
    assert_eq!(w, ["lib/state.ts#S.charged"]);
}

#[test]
fn without_the_event_rule_the_email_is_invisible() {
    let d = scratch("no-rule");
    std::fs::remove_file(d.join("flow.toml")).unwrap();
    let l = flowcontrol::lift(&d, "flows").unwrap();
    assert!(!steps(&l, "POST /api/upgrade").contains(&"onPlanChanged".to_string()));
}

#[test]
fn lifting_is_deterministic() {
    let a = flowcontrol::lift(&demo(), "flows").unwrap();
    let b = flowcontrol::lift(&demo(), "flows").unwrap();
    assert_eq!(a.out.files, b.out.files);
    assert_eq!(a.out.fingerprint, b.out.fingerprint);
}

#[test]
fn placing_the_line_by_hand_clears_the_gate() {
    let d = scratch("assign");
    let toml = std::fs::read_to_string(d.join("flow.toml")).unwrap()
        + "\n[[assign]]\nfile = \"lib/mailer.ts\"\nline = 'on(\"plan_changed\", onPlanChanged);'\nto = \"onPlanChanged\"\n";
    std::fs::write(d.join("flow.toml"), toml).unwrap();
    let l = flowcontrol::lift(&d, "flows").unwrap();
    assert!(l.out.stats.unplaced.is_empty());
    assert_eq!(l.out.stats.placed_by_hand, 1);
    assert!(l.out.files["_slices.yaml"].contains("placed_by_hand: \"14\""));
}

#[test]
fn a_code_edit_changes_only_the_flows_that_run_it() {
    let d = scratch("drift");
    let l = flowcontrol::lift(&d, "flows").unwrap();
    flowcontrol::write(&d.join("flows"), &l.out.files).unwrap();
    let disk = flowcontrol::read_existing(&d.join("flows")).unwrap();
    assert!(flowcontrol::drift(&l.out.files, &disk).is_clean());

    // Edit one shared function: the loyalty discount from the flow-control prototype.
    let p = d.join("lib/billing.ts");
    let src = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, src.replace("+ amount;", "+ Math.round(amount * 80) / 100;")).unwrap();
    let l2 = flowcontrol::lift(&d, "flows").unwrap();
    let dr = flowcontrol::drift(&l2.out.files, &disk);
    assert_eq!(dr.changed, ["_index.yaml", "_slices.yaml", "post-api-upgrade.flow.yaml"]);
    assert!(dr.added.is_empty() && dr.removed.is_empty());

    // applyPlanChange is shared by upgrade and cancel, so both flows move.
    std::fs::write(&p, src.replace("interval: \"month\" } };", "interval: S.interval ?? \"month\" } };")).unwrap();
    let l3 = flowcontrol::lift(&d, "flows").unwrap();
    let dr = flowcontrol::drift(&l3.out.files, &disk);
    assert_eq!(dr.changed, ["_index.yaml", "_slices.yaml", "post-api-cancel.flow.yaml", "post-api-upgrade.flow.yaml"]);
}

#[test]
fn the_view_embeds_every_journey() {
    let l = flowcontrol::lift(&demo(), "flows").unwrap();
    let html = flowcontrol::view::render(&l, "demo", &Default::default());
    assert!(!html.contains("/*DATA*/null"));
    for f in &l.flows {
        assert!(html.contains(&format!("\"slug\":\"{}\"", f.slug)));
    }
}

fn git(d: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(d).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(d: &Path, msg: &str) -> String {
    git(d, &["add", "-A"]);
    git(d, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", msg]);
    git(d, &["rev-parse", "HEAD"])
}

fn affected_between(d: &Path, a: &str, b: &str) -> flowcontrol::affected::Report {
    use flowcontrol::source::Source;
    let rev = |r: &str| Source::Rev { root: d.to_path_buf(), rev: r.to_string() };
    let la = flowcontrol::lift_source(&rev(a), "flows").unwrap();
    let lb = flowcontrol::lift_source(&rev(b), "flows").unwrap();
    flowcontrol::affected::affected(&la, &lb)
}

#[test]
fn affected_lists_only_the_journeys_a_commit_touched() {
    use flowcontrol::affected::How;
    let d = scratch("affected");
    git(&d, &["init", "-q"]);
    let base = commit(&d, "base");
    let ids = |r: &flowcontrol::affected::Report| r.flows.iter().map(|f| f.id.clone()).collect::<Vec<_>>();

    // Nothing changed.
    let r = affected_between(&d, &base, &base);
    assert!(r.is_empty());
    assert_eq!(r.unchanged, 6);

    // A function only the upgrade journey runs.
    let p = d.join("lib/billing.ts");
    let src = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, src.replace("+ amount;", "+ Math.round(amount * 80) / 100;")).unwrap();
    let one = commit(&d, "discount");
    let r = affected_between(&d, &base, &one);
    assert_eq!(ids(&r), ["POST /api/upgrade"]);
    assert_eq!(r.flows[0].how, How::Changed);
    assert_eq!(r.flows[0].edited, ["lib/billing.ts#chargeCard"]);
    assert!(r.flows[0].writes.contains(&"lib/state.ts#S.charged".to_string()));
    assert_eq!(r.unchanged, 5);

    // A function shared by upgrade and cancel moves both.
    std::fs::write(&p, src.replace("interval: \"month\" } };", "interval: S.interval ?? \"month\" } };")).unwrap();
    let two = commit(&d, "interval");
    assert_eq!(ids(&affected_between(&d, &base, &two)), ["POST /api/cancel", "POST /api/upgrade"]);

    // Code no flow runs, and load-time wiring with no owner, are reported apart from the flows.
    std::fs::write(&p, &src).unwrap();
    let pl = d.join("lib/plugins.ts");
    let ps = std::fs::read_to_string(&pl).unwrap();
    std::fs::write(&pl, ps.replace("legacyExport", "legacyExport2")).unwrap();
    let ml = d.join("lib/mailer.ts");
    let ms = std::fs::read_to_string(&ml).unwrap();
    std::fs::write(&ml, ms.replace("on(\"plan_changed\", onPlanChanged);", "on(\"plan_changed\", onPlanChanged); // wired")).unwrap();
    let three = commit(&d, "loose");
    let r = affected_between(&d, &base, &three);
    assert!(r.flows.is_empty(), "{:?}", ids(&r));
    assert_eq!(r.loose, ["lib/plugins.ts#legacyExport", "lib/plugins.ts#legacyExport2"]);
    assert_eq!(r.unplaced, ["lib/mailer.ts"]);
    assert!(r.uncertain());
}
