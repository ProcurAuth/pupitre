// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the options route as a function (3.16): the query, the display questions, the order,
//! the search, the limit, the budget and the cursor.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod access_support;

use std::collections::BTreeMap;

use access_support::*;
use pupitre::access::options::{OptionsError, OptionsTree, make_cursor, options_route, parse_query};
use pupitre::access::{Access, AnyAccess, AuthzenAccess, OpenAccess};
use pupitre_core::ids::Subject;
use pupitre_core::json::to_compact;
use pupitre_core::node::{Node, NodeType};
use serde_json::{Value, json};

/// A tree with ids and labels. The order of the children is not the byte order on purpose.
struct Tree {
    environments: Vec<(String, String)>,
    units: BTreeMap<String, Vec<(String, String)>>,
    hosts: BTreeMap<String, Vec<(String, String)>>,
}

impl Tree {
    fn small() -> Tree {
        let pairs =
            |list: &[(&str, &str)]| list.iter().map(|(i, l)| ((*i).to_owned(), (*l).to_owned())).collect::<Vec<_>>();
        Tree {
            environments: pairs(&[("staging", "Staging"), ("production", "Production"), ("dev", "Development")]),
            units: BTreeMap::from([
                ("production".to_owned(), pairs(&[("relay", "Relay"), ("api", "API")])),
                ("staging".to_owned(), pairs(&[("relay", "Relay")])),
            ]),
            hosts: BTreeMap::from([(
                "production/relay".to_owned(),
                pairs(&[("host-b", "host-b"), ("host-a", "host-a")]),
            )]),
        }
    }

    fn many(count: usize) -> Tree {
        let environments = (0..count).map(|i| (format!("e{i:02}"), format!("Environment {i}"))).collect();
        Tree { environments, units: BTreeMap::new(), hosts: BTreeMap::new() }
    }
}

impl OptionsTree for Tree {
    fn children(&self, parent: Option<&Node>, child_type: NodeType) -> Vec<(String, String)> {
        match (parent, child_type) {
            (None, NodeType::Environment) => self.environments.clone(),
            (Some(Node::Environment(e)), NodeType::Unit) => self.units.get(e.as_str()).cloned().unwrap_or_default(),
            (Some(Node::Unit(e, u)), NodeType::Host) => {
                self.hosts.get(&format!("{e}/{u}")).cloned().unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

fn start(
    policy: impl Fn(&access_support::Question) -> Answer + Send + Sync + 'static,
) -> (FakePdp, AnyAccess, Subject) {
    let pdp = FakePdp::start(policy);
    start_with(pdp, |_| {})
}

fn start_with(pdp: FakePdp, edit: impl FnOnce(&mut pupitre::config::AuthzenConfig)) -> (FakePdp, AnyAccess, Subject) {
    let mut c = config(&pdp.url());
    edit(&mut c);
    let access = AuthzenAccess::new(&c, true, 100).unwrap();
    let t = token(&ed_key(), Some("ed-1"), "alice");
    let subject = access.authenticate(&[t.as_bytes()]).unwrap();
    (pdp, AnyAccess::Authzen(Box::new(access)), subject)
}

async fn route(access: &AnyAccess, subject: &Subject, query: &str, tree: &Tree) -> Result<Value, OptionsError> {
    options_route(access, subject, query, tree, true, 200, 50)
        .await
        .map(|j| serde_json::from_slice(&to_compact(&j)).unwrap())
}

fn ids(answer: &Value) -> Vec<String> {
    answer["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect()
}

fn invalid<T: std::fmt::Debug>(result: Result<T, OptionsError>) -> String {
    match result {
        Err(OptionsError::InvalidRequest(text)) => text,
        other => panic!("expected an invalid request, found {other:?}"),
    }
}

/// The subject sees the environments whose id is in `visible`, by the first action.
fn sees(visible: &'static [&'static str]) -> impl Fn(&access_support::Question) -> Answer + Send + Sync + 'static {
    move |q| {
        let node = q.resource.rsplit('/').next().unwrap_or("");
        if visible.iter().any(|v| node.ends_with(v)) { Answer::yes() } else { Answer::no() }
    }
}

// ---- The answer ------------------------------------------------------------------------------

#[tokio::test]
async fn p3_16_the_first_level_lists_the_environments_in_the_byte_order_of_the_id() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let answer = route(&access, &s, "type=environment", &Tree::small()).await.unwrap();
    assert_eq!(ids(&answer), ["dev", "production", "staging"]);
    assert_eq!(answer["next"], Value::Null);
    assert_eq!(answer["items"][1], json!({"has_children": true, "id": "production", "label": "Production"}));
}

#[tokio::test]
async fn p3_16_the_compact_bytes_of_the_answer() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let answer = options_route(&access, &s, "type=environment&limit=1", &Tree::small(), true, 200, 50).await.unwrap();
    let text = String::from_utf8(to_compact(&answer)).unwrap();
    assert!(
        text.starts_with(r#"{"items":[{"has_children":true,"id":"dev","label":"Development"}],"next":""#),
        "{text}"
    );
}

#[tokio::test]
async fn p3_16_the_units_and_the_hosts_with_their_has_children() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::small();
    let units = route(&access, &s, "parent=environment%3Aproduction&type=unit", &tree).await.unwrap();
    assert_eq!(ids(&units), ["api", "relay"]);
    assert_eq!(units["items"][0]["has_children"], true);
    let hosts = route(&access, &s, "parent=environment%3Aproduction%2Funit%3Arelay&type=host", &tree).await.unwrap();
    assert_eq!(ids(&hosts), ["host-a", "host-b"]);
    assert_eq!(hosts["items"][0]["has_children"], false);
}

#[tokio::test]
async fn p3_16_a_parent_that_does_not_exist_gives_no_item() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let answer = route(&access, &s, "parent=environment%3Aghost&type=unit", &Tree::small()).await.unwrap();
    assert_eq!(answer, json!({"items": [], "next": null}));
}

#[tokio::test]
async fn p3_16_only_the_children_that_the_subject_can_see_are_listed() {
    let (pdp, access, s) = start(sees(&["production"]));
    let answer = route(&access, &s, "type=environment", &Tree::small()).await.unwrap();
    assert_eq!(ids(&answer), ["production"]);
    assert_eq!(answer["next"], Value::Null);
    // Each question has the match `subtree` and no usage.
    assert!(pdp.questions().iter().all(|q| q.match_mode == "subtree" && !q.usage));
}

#[tokio::test]
async fn p3_16_each_child_is_asked_with_the_published_actions_in_the_order_of_3_9() {
    let (pdp, access, s) = start(|_| Answer::no());
    let answer = route(&access, &s, "parent=environment%3Aproduction%2Funit%3Arelay&type=host&limit=1", &Tree::small())
        .await
        .unwrap();
    assert!(ids(&answer).is_empty());
    // The first host, hidden: the 6 actions that list the type `host`, in order, with no secret.write
    // and no inventory.write.
    let asked: Vec<String> =
        pdp.questions().iter().filter(|q| q.resource.ends_with("host:host-a")).map(|q| q.action.clone()).collect();
    assert_eq!(asked, ["config.read", "config.write", "run.read", "run.check", "run.apply", "command.run"]);
}

#[tokio::test]
async fn p3_16_the_mode_open_lists_everything_and_asks_nothing() {
    let access = AnyAccess::Open(OpenAccess);
    let answer = route(&access, &Subject::anonymous(), "type=environment", &Tree::small()).await.unwrap();
    assert_eq!(ids(&answer), ["dev", "production", "staging"]);
}

#[tokio::test]
async fn p3_16_a_failure_of_the_pdp_gives_an_error_never_an_empty_list() {
    let (pdp, access, s) = start(|_| Answer::yes());
    pdp.set_mode(Mode::Status(500));
    assert_eq!(route(&access, &s, "type=environment", &Tree::small()).await, Err(OptionsError::PdpUnavailable));
    pdp.set_mode(Mode::Garbage);
    assert_eq!(route(&access, &s, "type=environment", &Tree::small()).await, Err(OptionsError::PdpUnavailable));
}

// ---- The query -------------------------------------------------------------------------------

#[tokio::test]
async fn p5_7_the_rules_of_the_query_string() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::small();
    // No `type`.
    assert!(invalid(route(&access, &s, "", &tree).await).contains("`type`"));
    assert!(invalid(route(&access, &s, "limit=3", &tree).await).contains("`type`"));
    // A parameter two times.
    assert!(
        invalid(route(&access, &s, "type=environment&type=environment", &tree).await).contains("more than one time")
    );
    assert!(invalid(route(&access, &s, "type=environment&q=a&q=b", &tree).await).contains("`q`"));
    // The type is not the type of the children of the parent.
    assert!(invalid(route(&access, &s, "type=unit", &tree).await).contains("`type`"));
    assert!(invalid(route(&access, &s, "parent=environment%3Aproduction&type=host", &tree).await).contains("`type`"));
    assert!(
        invalid(route(&access, &s, "parent=environment%3Aproduction%2Funit%3Arelay&type=unit", &tree).await)
            .contains("`type`")
    );
    assert!(invalid(route(&access, &s, "type=service", &tree).await).contains("`type`"));
    // A parent below a host has no children.
    assert!(
        invalid(
            route(&access, &s, "parent=environment%3Aproduction%2Funit%3Arelay%2Fhost%3Ahost-a&type=host", &tree).await
        )
        .contains("`type`")
    );
    // An unknown parameter has no effect.
    let ok = route(&access, &s, "type=environment&colour=red&limit=1", &tree).await.unwrap();
    assert_eq!(ids(&ok), ["dev"]);
}

#[tokio::test]
async fn p5_7_each_percent_escape_is_decoded_one_time_and_a_plus_is_not_a_space() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::small();
    // The parent with its separators encoded (the form of P 5.7).
    assert!(route(&access, &s, "parent=environment%3Aproduction&type=unit", &tree).await.is_ok());
    // A colon that the caller did not encode, a lower-case escape, a bad escape and a raw plus.
    for bad in [
        "parent=environment:production&type=unit",
        "parent=environment%3aproduction&type=unit",
        "parent=environment%3&type=unit",
        "parent=environment%ZZproduction&type=unit",
        "type=environment&q=a+b",
        "type=environment&q=%C3%28",
    ] {
        invalid(route(&access, &s, bad, &tree).await);
    }
    // A search with an encoded space and an encoded plus.
    let mut tree = Tree::small();
    tree.environments.push(("plus".to_owned(), "A+B".to_owned()));
    tree.environments.push(("space".to_owned(), "A B".to_owned()));
    assert_eq!(ids(&route(&access, &s, "type=environment&q=a%2Bb", &tree).await.unwrap()), ["plus"]);
    assert_eq!(ids(&route(&access, &s, "type=environment&q=a%20b", &tree).await.unwrap()), ["space"]);
    // A parent that is not a node path.
    for bad in [
        "parent=production&type=unit",
        "parent=unit%3Arelay&type=unit",
        "parent=&type=unit",
        "parent=environment%3AProduction&type=unit",
    ] {
        invalid(route(&access, &s, bad, &tree).await);
    }
}

#[tokio::test]
async fn p3_16_the_search_keeps_the_children_whose_id_or_label_holds_the_text() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::small();
    // The id, with no case for ASCII letters.
    assert_eq!(ids(&route(&access, &s, "type=environment&q=PROD", &tree).await.unwrap()), ["production"]);
    // The label.
    assert_eq!(ids(&route(&access, &s, "type=environment&q=development", &tree).await.unwrap()), ["dev"]);
    assert_eq!(ids(&route(&access, &s, "type=environment&q=zzz", &tree).await.unwrap()), Vec::<String>::new());
    // 256 bytes are allowed, 257 are not.
    let q256 = "a".repeat(256);
    assert!(route(&access, &s, &format!("type=environment&q={q256}"), &tree).await.is_ok());
    assert!(invalid(route(&access, &s, &format!("type=environment&q={q256}a"), &tree).await).contains("256"));
}

#[tokio::test]
async fn p3_16_the_questions_do_not_depend_on_a_search_that_matches_a_hidden_name() {
    // `production` is hidden. A search that holds its name must not change the questions to the
    // PDP: else the time or the count shows that a hidden name holds the text.
    let tree = Tree::small();
    let mut counts = Vec::new();
    for q in ["production", "PRODUCT", "zzz", "", "staging"] {
        let (pdp, access, s) = start(sees(&["dev", "staging"]));
        let page = route(&access, &s, &format!("type=environment&q={q}"), &tree).await.unwrap();
        let shown = ids(&page);
        assert!(!shown.contains(&"production".to_owned()), "{q}: {shown:?}");
        counts.push((q, pdp.questions().len(), shown));
    }
    let hidden_match = counts[0].1;
    assert!(hidden_match > 0);
    assert_eq!(counts[1].1, hidden_match, "{counts:?}");
    assert_eq!(counts[2].1, hidden_match, "the same count for a search with no match: {counts:?}");
    assert_eq!(counts[3].1, hidden_match, "{counts:?}");
    assert_eq!(counts[4].1, hidden_match, "{counts:?}");
    // The search still keeps only the visible children that hold the text.
    assert_eq!(counts[4].2, ["staging"]);
    assert!(counts[0].2.is_empty() && counts[2].2.is_empty());
}

#[tokio::test]
async fn p3_16_a_search_pages_over_the_visible_children_with_the_cursor() {
    // 10 environments, the odd ones hidden. The search `environment` holds each visible one.
    // With `limit=2` the pages are `e00 e02`, `e04 e06`, `e08`, and a cursor never names a hidden id.
    let tree = Tree::many(10);
    let (_pdp, access, s) = start(|q| {
        let id = q.resource.rsplit("environment:").next().unwrap();
        let n: u32 = id.trim_start_matches('e').parse().unwrap();
        if n.is_multiple_of(2) { Answer::yes() } else { Answer::no() }
    });
    let mut query = "type=environment&q=ENVIRONMENT&limit=2".to_owned();
    let mut pages = Vec::new();
    loop {
        let page = route(&access, &s, &query, &tree).await.unwrap();
        pages.push(ids(&page));
        match page["next"].as_str() {
            Some(cursor) => query = format!("type=environment&q=ENVIRONMENT&limit=2&after={cursor}"),
            None => break,
        }
        assert!(pages.len() < 20);
    }
    let all: Vec<String> = pages.concat();
    assert_eq!(all, ["e00", "e02", "e04", "e06", "e08"], "{pages:?}");
    // A search that holds a few labels: the pages hold only these, and the walk goes on over the hidden ones.
    let page = route(&access, &s, "type=environment&q=Environment%204", &tree).await.unwrap();
    assert_eq!(ids(&page), ["e04"]);
}

#[tokio::test]
async fn p3_16_the_limit_is_from_1_to_the_page_bound_and_is_never_changed() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::many(60);
    // Absent: the page bound (50 here).
    let page = route(&access, &s, "type=environment", &tree).await.unwrap();
    assert_eq!(ids(&page).len(), 50);
    assert!(page["next"].is_string());
    assert_eq!(ids(&route(&access, &s, "type=environment&limit=50", &tree).await.unwrap()).len(), 50);
    assert_eq!(ids(&route(&access, &s, "type=environment&limit=1", &tree).await.unwrap()), ["e00"]);
    // Above the bound, zero, a sign, a fraction, an empty value and an overflow: 400.
    for bad in ["51", "0", "-1", "+1", "1.5", "", "1e1", "0x10", "99999999999", "4294967296", "01a"] {
        let e = invalid(route(&access, &s, &format!("type=environment&limit={bad}"), &tree).await);
        assert!(e.contains("`limit`"), "{bad}: {e}");
    }
    // The bound of the configuration applies: with a bound of 10, 11 is refused.
    let r = options_route(&access, &s, "type=environment&limit=11", &tree, true, 200, 10).await;
    assert!(invalid(r).contains("1 to 10"));
    let r = options_route(&access, &s, "type=environment", &tree, true, 200, 10).await.unwrap();
    assert_eq!(ids(&serde_json::from_slice(&to_compact(&r)).unwrap()).len(), 10);
}

#[test]
fn p3_16_the_query_parser_gives_the_typed_query() {
    let q = parse_query("parent=environment%3Aproduction&type=unit&q=re&limit=7", 50).unwrap();
    assert_eq!(q.parent.unwrap().path(), "environment:production");
    assert_eq!(q.child_type, NodeType::Unit);
    assert_eq!((q.q.as_deref(), q.limit, q.after), (Some("re"), 7, None));
}

// ---- The cursor ------------------------------------------------------------------------------

#[tokio::test]
async fn p3_16_the_pages_follow_one_another_with_the_cursor() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::many(7);
    let mut seen = Vec::new();
    let mut query = "type=environment&limit=3".to_owned();
    let mut pages = 0;
    loop {
        let answer = route(&access, &s, &query, &tree).await.unwrap();
        pages += 1;
        seen.extend(ids(&answer));
        match answer["next"].as_str() {
            Some(next) => query = format!("type=environment&limit=3&after={next}"),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ["e00", "e01", "e02", "e03", "e04", "e05", "e06"]);
}

#[tokio::test]
async fn p3_16_the_cursor_is_the_text_of_the_specification_and_holds_no_hidden_id() {
    // The subject sees only `e01` and `e05`. Page 1 has `limit=1`.
    let (_pdp, access, s) = start(sees(&["e01", "e05"]));
    let tree = Tree::many(8);
    let page = route(&access, &s, "type=environment&limit=1", &tree).await.unwrap();
    assert_eq!(ids(&page), ["e01"]);
    let cursor = page["next"].as_str().unwrap();
    // `<type> LF <parent> LF <last visible id> LF <hidden children after it>`, base64url, no padding.
    assert_eq!(cursor, make_cursor(NodeType::Environment, "", Some("e01"), 0));
    assert!(!cursor.contains('=') && !cursor.contains('+') && !cursor.contains('/'));
    // The page 2 starts after `e01`, then it has `e05` after 3 hidden children.
    let page2 = route(&access, &s, &format!("type=environment&limit=1&after={cursor}"), &tree).await.unwrap();
    assert_eq!(ids(&page2), ["e05"]);
    let cursor2 = page2["next"].as_str().unwrap();
    assert_eq!(cursor2, make_cursor(NodeType::Environment, "", Some("e05"), 0));
    // The page 3 has no item and no cursor: `e06` and `e07` are hidden.
    let page3 = route(&access, &s, &format!("type=environment&limit=1&after={cursor2}"), &tree).await.unwrap();
    assert_eq!(page3, json!({"items": [], "next": null}));
    // No cursor holds the id of a hidden child.
    for c in [cursor, cursor2] {
        let decoded = pupitre::access::options::parse_query(&format!("type=environment&after={c}"), 50).unwrap();
        let (id, _) = decoded.after.unwrap();
        assert!(id == "e01" || id == "e05");
    }
}

#[tokio::test]
async fn p3_16_a_page_with_no_visible_item_keeps_the_place_and_gives_a_cursor() {
    // 12 children, one seen at the end. A budget of 8 questions examines one hidden child (8 actions).
    let (_pdp, access, s) = start(sees(&["e11"]));
    let tree = Tree::many(12);
    let mut query = "type=environment".to_owned();
    let mut all = Vec::new();
    let mut requests = 0;
    loop {
        let answer = options_route(&access, &s, &query, &tree, true, 8, 50).await.unwrap();
        let answer: Value = serde_json::from_slice(&to_compact(&answer)).unwrap();
        requests += 1;
        all.extend(ids(&answer));
        match answer["next"].as_str() {
            Some(next) => {
                // The cursor of a page with no visible item holds no id of a child.
                query = format!("type=environment&after={next}");
            }
            None => break,
        }
        assert!(requests < 100, "the cursor made no progress");
    }
    assert_eq!(all, ["e11"]);
    // Each request ended at least one child, so there are at most 12 requests.
    assert!(requests <= 12, "{requests}");
    assert!(requests > 1, "the budget never stopped the walk");
}

#[tokio::test]
async fn p3_16_one_request_asks_at_most_the_budget_of_questions() {
    let (pdp, access, s) = start(|_| Answer::no());
    let tree = Tree::many(30);
    for budget in [8u32, 20, 64] {
        pdp.clear();
        let answer = options_route(&access, &s, "type=environment", &tree, true, budget, 50).await.unwrap();
        let asked = u32::try_from(pdp.questions().len()).unwrap();
        assert!(asked <= budget, "{asked} questions with a budget of {budget}");
        assert!(asked > budget - 8, "the walk stopped early: {asked}");
        let answer: Value = serde_json::from_slice(&to_compact(&answer)).unwrap();
        // The answer can have no item and a cursor.
        assert!(ids(&answer).is_empty());
        assert!(answer["next"].is_string());
    }
}

#[tokio::test]
async fn p3_16_a_cursor_that_the_route_did_not_make_for_this_type_and_parent_is_refused() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::small();
    let own = make_cursor(NodeType::Environment, "", Some("dev"), 0);
    assert!(route(&access, &s, &format!("type=environment&after={own}"), &tree).await.is_ok());
    let bad = [
        // Another type or another parent.
        make_cursor(NodeType::Unit, "", Some("dev"), 0),
        make_cursor(NodeType::Environment, "environment:production", Some("dev"), 0),
        // An id that is not an id of the type, a count that is not canonical.
        make_cursor(NodeType::Environment, "", Some("Dev"), 0),
        pupitre_access_text("environment\n\ndev\n01"),
        pupitre_access_text("environment\n\ndev\n-1"),
        pupitre_access_text("environment\n\ndev\n"),
        pupitre_access_text("environment\n\ndev\n1234567890"),
        pupitre_access_text("environment\n\ndev"),
        pupitre_access_text("environment\n\ndev\n1\nmore"),
        // Not base64url, padding, not text.
        "not a cursor".to_owned(),
        format!("{own}="),
        "AAAA".to_owned(),
        String::new(),
    ];
    for cursor in bad {
        let encoded: String = cursor
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let e = invalid(route(&access, &s, &format!("type=environment&after={encoded}"), &tree).await);
        assert!(e.contains("`after`"), "{cursor:?}: {e}");
    }
    // A cursor for the units of `production` is not a cursor for the units of `staging`.
    let for_production = make_cursor(NodeType::Unit, "environment:production", Some("api"), 0);
    assert!(
        route(&access, &s, &format!("parent=environment%3Aproduction&type=unit&after={for_production}"), &tree)
            .await
            .is_ok()
    );
    invalid(route(&access, &s, &format!("parent=environment%3Astaging&type=unit&after={for_production}"), &tree).await);
    // 2,048 bytes are allowed, 2,049 are not.
    let long = "A".repeat(2049);
    assert!(invalid(route(&access, &s, &format!("type=environment&after={long}"), &tree).await).contains("2048"));
}

/// The base64url text of a cursor body that the route did not make.
fn pupitre_access_text(body: &str) -> String {
    let mut text = String::new();
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for chunk in body.as_bytes().chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            text.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    text
}

#[tokio::test]
async fn p3_16_the_walk_skips_what_the_cursor_says_and_filters_with_the_search() {
    let (_pdp, access, s) = start(|_| Answer::yes());
    let tree = Tree::many(10);
    // After `e02` with 2 hidden children skipped: the page starts at `e05`.
    let cursor = make_cursor(NodeType::Environment, "", Some("e02"), 2);
    let page = route(&access, &s, &format!("type=environment&limit=2&after={cursor}"), &tree).await.unwrap();
    assert_eq!(ids(&page), ["e05", "e06"]);
    // The search applies to the visible children, after the cursor.
    let page = route(&access, &s, "type=environment&q=environment%201", &tree).await.unwrap();
    assert_eq!(ids(&page), ["e01"]);
}
