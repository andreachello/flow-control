//! Journeys are proposed once and then belong to the people who read them, so an answer from Claude
//! can only add to what is there, and only with requests that exist.

use flowcontrol::ids::Ids;
use flowcontrol::journeys::{apply, unplaced, Journey, Journeys, BEHIND, ICONS};
use serde_json::json;

fn journey(title: &str, requests: &[&str]) -> Journey {
    Journey { product: None, title: title.into(), icon: "star".into(), requests: requests.iter().map(|r| r.to_string()).collect(), stops: vec![] }
}

#[test]
fn an_answer_cannot_invent_a_request_or_move_one_twice() {
    let mut have = Journeys::default();
    let answer = json!({"journeys": [
        {"title": "Log in", "icon": "enter", "requests": ["page /login", "action login", "action imagined"]},
        {"title": "Sign up", "icon": "not an icon", "requests": ["page /login", "action signup"]},
        {"title": "Nothing here", "icon": "star", "requests": []},
    ]});
    let n = apply(&mut have, &answer, &["page /login", "action login", "action signup", "page /left"]);
    assert_eq!(n, 3);
    assert_eq!(have.journeys.len(), 2);
    assert_eq!(have.journeys[0].requests, ["page /login", "action login"]);
    assert_eq!(have.journeys[1].requests, ["action signup"]);
    assert_eq!(have.journeys[1].icon, "star", "an icon the page cannot draw is replaced");
}

#[test]
fn a_second_run_adds_to_the_journeys_people_already_know() {
    let mut have = Journeys { journeys: vec![journey("Log in", &["page /login"]), journey("Pay", &["action pay"])], ..Journeys::default() };
    let answer = json!({"journeys": [
        {"title": "log in", "icon": "plus", "requests": ["action forgot"]},
        {"title": "Get help", "icon": "chat", "requests": ["page /help"]},
    ]});
    assert_eq!(apply(&mut have, &answer, &["action forgot", "page /help"]), 2);
    assert_eq!(have.journeys[0].title, "Log in", "the title a person kept stands");
    assert_eq!(have.journeys[0].icon, "star");
    assert_eq!(have.journeys[0].requests, ["page /login", "action forgot"]);
    assert_eq!(have.journeys[1].title, "Pay", "existing journeys keep their place, so their colour");
    assert_eq!(have.journeys[2].title, "Get help");
}

#[test]
fn a_journey_keeps_a_request_through_a_rename() {
    let have = Journeys { journeys: vec![journey("Pay", &["POST /api/pay"])], ..Journeys::default() };
    let (ids, bad) = Ids::parse("{\"id\":\"j_1\",\"key\":\"POST /api/checkout\"}\n{\"id\":\"j_1\",\"was\":\"POST /api/pay\"}\n");
    assert!(bad.is_empty());
    let places = have.places(&ids);
    assert_eq!(places.of("j_1", "POST /api/checkout"), Some(0), "the new name is the same request");
    assert_eq!(places.of("j_other", "POST /api/elsewhere"), None);
}

#[test]
fn only_requests_without_a_journey_go_to_claude() {
    let d = std::env::temp_dir().join(format!("flow-journeys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for r in ["a", "b"] {
        std::fs::create_dir_all(d.join(format!("app/api/{r}"))).unwrap();
        std::fs::write(d.join(format!("app/api/{r}/route.ts")), "export async function POST() {\n  return new Response('1');\n}\n").unwrap();
    }
    let l = flowcontrol::lift(&d, "flows").unwrap();
    let have = Journeys { journeys: vec![journey("Do a", &["POST /api/a"])], ..Journeys::default() };
    assert_eq!(unplaced(&l, &have, &l.ids), ["POST /api/b"]);
    let p = flowcontrol::journeys::prompt(&l, &have, &["POST /api/b"]);
    assert!(p.contains("## POST /api/b") && !p.contains("## POST /api/a"), "{p}");
    assert!(p.contains("Do a"), "the journeys that exist are offered first");
}

#[test]
fn a_change_is_about_what_its_narrowest_code_reaches() {
    let set = |v: &[usize]| v.iter().copied().collect::<std::collections::BTreeSet<_>>();
    // A page fix beside a shared layout is about the page.
    assert_eq!(flowcontrol::journeys::focus(&[set(&[0, 1, 2, 3, 4]), set(&[3])]), set(&[3]));
    // Code no journey runs says nothing.
    assert_eq!(flowcontrol::journeys::focus(&[set(&[]), set(&[1, 2])]), set(&[1, 2]));
    // Only shared code changed: it is about all of it.
    assert_eq!(flowcontrol::journeys::focus(&[set(&[0, 1, 2])]), set(&[0, 1, 2]));
}

#[test]
fn a_title_must_fit_a_row_and_a_clause_must_name_a_step_that_changed() {
    use flowcontrol::titles::{take, Known, Titles, QUIET};
    let mut t = Titles::new();
    let known: Known = [("abc123aaaa", "a.ts#pay"), ("def456bbbb", "b.ts#x"), ("aaa000cccc", "c.ts#y")].iter().map(|(k, s)| (k.to_string(), [s.to_string()].into_iter().collect())).collect();
    let answer = json!({"titles": [
        {"id": "abc123", "title": "Pay yearly for the price of ten months.", "parts": [
            {"say": "People can pay yearly,", "line": "People can now pay once a year.", "at": "Pick a plan.", "before": "People paid monthly.", "after": "People can pay yearly.", "step": "a.ts#pay"},
            {"say": "and something imagined.", "step": "a.ts#never"},
        ]},
        {"id": "fff999", "title": "A change that was never saved.", "parts": []},
        {"id": "def456", "title": "This one goes on far too long to fit on a single row of the list of changes.", "parts": []},
        {"id": "aaa000", "title": QUIET, "parts": [{"say": "Nothing, but said anyway.", "step": "c.ts#y"}]},
    ]});
    assert_eq!(take(&mut t, &known, &answer), 2);
    let paid = &t["abc123aaaa"];
    assert_eq!(paid.parts.len(), 1, "a clause about a step that did not change is dropped");
    assert_eq!(paid.parts[0].say, "People can pay yearly,");
    assert_eq!(paid.parts[0].line, "People can now pay once a year.");
    assert_eq!(paid.parts[0].at, "Pick a plan.");
    assert_eq!((paid.parts[0].before.as_str(), paid.parts[0].after.as_str()), ("People paid monthly.", "People can pay yearly."));
    assert!(t["aaa000cccc"].parts.is_empty(), "a change nobody notices has nothing to say");
    assert_eq!(t.len(), 2);
}

#[test]
fn a_stop_has_to_rest_on_requests_of_its_journey() {
    use flowcontrol::journeys::take_stops;
    let mut j = journey("Pay", &["page /pay", "action pay"]);
    let answer = json!({"stops": [
        {"say": "Choose a plan.", "requests": ["page /pay"]},
        {"say": "Pay for it.", "requests": ["action pay", "action imagined"]},
        {"say": "Do something no request does.", "requests": ["action imagined"]},
    ]});
    assert_eq!(take_stops(&mut j, &answer), 2);
    assert_eq!(j.stops[1].requests, ["action pay"]);

    let mut k = journey("Pay", &["page /pay", "action pay"]);
    assert_eq!(take_stops(&mut k, &json!({"stops": [{"say": "One stop.", "requests": ["page /pay"]}]})), 0, "one stop of several requests is no journey");
    assert!(k.stops.is_empty());
}

#[test]
fn the_page_can_draw_every_icon() {
    let page = include_str!("../src/serve.html");
    let shapes = &page[page.find("const SHAPE").unwrap()..page.find("const TINT").unwrap()];
    for i in ICONS {
        assert!(shapes.contains(&format!("{i}:\"")), "no shape for {i}");
    }
    assert!(ICONS.contains(&BEHIND));
}

mod products {
    use flowcontrol::journeys::{group, seed, unplaced, Journey, Journeys, Product, BEHIND};
    use flowcontrol::source::Source;
    use std::collections::BTreeMap;

    fn lift(files: &[(&str, &str)]) -> flowcontrol::Lift {
        let m: BTreeMap<String, String> = files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        flowcontrol::lift_source(&Source::Mem { root: "/nowhere".into(), files: m }, ".flow").unwrap()
    }

    fn monorepo() -> Vec<(&'static str, &'static str)> {
        vec![
            ("apps/web/next.config.ts", "export default {}\n"),
            ("apps/web/tsconfig.json", "{ /* apps have their own */ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./*\"] } } }\n"),
            ("apps/web/app/settings/page.tsx", "import { bill } from \"@/lib/bill\";\nexport default function Page() { return bill(); }\n"),
            ("apps/web/lib/bill.ts", "import { charge } from \"@acme/core\";\nexport function bill() { return charge(); }\n"),
            ("apps/internal/next.config.ts", "export default {}\n"),
            ("apps/internal/tsconfig.json", "{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./*\"] } } }\n"),
            ("apps/internal/app/settings/page.tsx", "import { audit } from \"@/lib/bill\";\nexport default function Page() { return audit(); }\n"),
            ("apps/internal/lib/bill.ts", "export function audit() { return 2; }\n"),
            ("packages/core/package.json", "{ \"name\": \"@acme/core\", \"main\": \"./dist/index.mjs\" }\n"),
            ("packages/core/src/index.ts", "export function charge() { return 1; }\n"),
            ("examples/demo/next.config.ts", "export default {}\n"),
            ("examples/demo/app/page.tsx", "export default function Page() { return 3; }\n"),
        ]
    }

    #[test]
    fn apps_in_one_repo_each_keep_their_own_routes() {
        let l = lift(&monorepo());
        let ids: Vec<&str> = l.flows.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"web: page /settings"), "{ids:?}");
        assert!(ids.contains(&"internal: page /settings"), "{ids:?}");
    }

    #[test]
    fn an_app_reaches_its_own_aliases_and_the_repos_packages() {
        let l = lift(&monorepo());
        let steps = |id: &str| -> Vec<String> {
            let f = l.flows.iter().find(|f| f.id == id).unwrap();
            f.steps.iter().map(|s| l.repo.slice(s.at).name.clone()).collect()
        };
        let web = steps("web: page /settings");
        assert!(web.iter().any(|s| s == "bill") && web.iter().any(|s| s == "charge"), "the same alias means this app's file, and a workspace package is followed to its source: {web:?}");
        assert!(steps("internal: page /settings").iter().any(|s| s == "audit"));
    }

    #[test]
    fn a_repo_of_one_app_at_the_root_reads_the_same_as_ever() {
        let l = lift(&[("next.config.ts", "export default {}\n"), ("app/settings/page.tsx", "export default function Page() { return 1; }\n")]);
        assert_eq!(l.flows[0].id, "page /settings");
    }

    #[test]
    fn apps_become_products_and_examples_stay_out() {
        let l = lift(&monorepo());
        let mut b = Journeys::default();
        assert!(seed(&l, &mut b));
        assert_eq!(b.products, [Product { name: "Internal".into(), path: "apps/internal".into() }, Product { name: "Web".into(), path: "apps/web".into() }]);
        assert_eq!(b.ignore, ["examples/demo"]);
        let todo = unplaced(&l, &b, &l.ids);
        assert!(!todo.iter().any(|t| t.starts_with("demo: ")), "{todo:?}");
    }

    #[test]
    fn a_project_already_grouped_is_not_pulled_apart() {
        let l = lift(&monorepo());
        let mut b = Journeys { journeys: vec![Journey { product: None, title: "Pay".into(), icon: "card".into(), requests: vec![], stops: vec![] }], ..Journeys::default() };
        assert!(!seed(&l, &mut b));
        assert!(b.products.is_empty());
    }

    #[test]
    fn code_in_no_product_goes_behind_the_scenes_without_asking() {
        let mut files = monorepo();
        files.push(("packages/core/src/save.ts", "\"use server\";\nexport async function save() { return 1; }\n"));
        let l = lift(&files);
        let mut b = Journeys { products: vec![Product { name: "None yet".into(), path: "apps/none".into() }], ignore: vec!["apps/".into(), "examples/".into()], ..Journeys::default() };
        let rep = group(&l, &mut b, &l.ids, "unused").unwrap();
        assert_eq!(rep.placed, 1);
        assert_eq!((b.journeys[0].product.as_deref(), b.journeys[0].icon.as_str()), (None, BEHIND));
        assert_eq!(b.journeys[0].requests, ["action save"]);
    }

    #[test]
    fn a_folder_matches_whole_names_only() {
        let b = Journeys { products: vec![Product { name: "Web".into(), path: "apps/web".into() }], ignore: vec!["examples/".into()], ..Journeys::default() };
        assert_eq!(b.product_of("apps/web/app/page.tsx"), Some("Web"));
        assert_eq!(b.product_of("apps/web-admin/app/page.tsx"), None);
        assert!(b.ignored("examples/a.ts") && !b.ignored("examples-old/a.ts"));
    }
}
