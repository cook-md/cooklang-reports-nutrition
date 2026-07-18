//! Locks the report-template contract for the shared allergens partial
//! (src/macros/allergens.jinja) that `NutritionExtension::register()`
//! exposes as the "allergens" template — the demo fixtures (and the
//! editor's nutri.md.jinja) `{% include %}` this exact file.
//!
//! The partial accepts *two* shapes fed by different call sites:
//! - the batch `cookmd_nutrition_client::AllergenSummary` rollup (`contains`,
//!   `unverified_ingredients`, `view`) from `aggregate_nutrition(...)`, and
//! - the per-item `cookmd_nutrition_client::AllergenBlock` (`status`, `contains`,
//!   `view` — no `unverified_ingredients`) from
//!   `nutrition_for_amount(...).allergens`.
//!
//! The leading `alg.status == "unverified"` guard exists only for the
//! per-item shape: an unaudited ingredient's block is
//! `{status: "unverified", contains: [], view: ...}`, and without that
//! guard the empty `contains` falls through to the "none identified"
//! branch — a false allergen-free claim. The summary shape never carries
//! `status`, so `Undefined == "unverified"` is falsy there and the guard
//! is a no-op for every existing summary-shape case below.

use cookmd_nutrition_client::{AllergenBlock, AllergenEntry, AllergenSummary};

const SNIPPET: &str = include_str!("../src/macros/allergens.jinja");

fn render(summary: &AllergenSummary) -> String {
    render_value(minijinja::Value::from_serialize(summary))
}

fn render_block(block: &AllergenBlock) -> String {
    render_value(minijinja::Value::from_serialize(block))
}

fn render_value(alg: minijinja::Value) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template("t", SNIPPET).unwrap();
    let ctx = minijinja::context! { alg => alg };
    env.get_template("t").unwrap().render(ctx).unwrap()
}

fn entry(class: &str, subtype: Option<&str>, label: &str) -> AllergenEntry {
    AllergenEntry {
        class: class.into(),
        subtype: subtype.map(str::to_string),
        label: label.into(),
    }
}

#[test]
fn fully_verified_with_tags() {
    let s = AllergenSummary {
        contains: vec![
            entry("gluten", Some("wheat"), "Wheat"),
            entry("milk", None, "Milk"),
        ],
        unverified_ingredients: vec![],
        view: "eu".into(),
    };
    assert_eq!(render(&s), "Contains: Wheat, Milk");
}

#[test]
fn partial_coverage_renders_floor_and_incomplete_line() {
    let s = AllergenSummary {
        contains: vec![entry("milk", None, "Milk")],
        unverified_ingredients: vec!["saffron".into(), "tamarind paste".into()],
        view: "eu".into(),
    };
    assert_eq!(
        render(&s),
        "Contains at least: Milk\nAllergen info incomplete for: saffron, tamarind paste"
    );
}

#[test]
fn fully_verified_empty_says_none_identified() {
    let s = AllergenSummary {
        contains: vec![],
        unverified_ingredients: vec![],
        view: "eu".into(),
    };
    assert_eq!(
        render(&s),
        "Contains: none of the 14 regulated allergens identified"
    );
}

#[test]
fn fda_verified_empty_scopes_claim_to_nine() {
    let s = AllergenSummary {
        contains: vec![],
        unverified_ingredients: vec![],
        view: "fda".into(),
    };
    assert_eq!(
        render(&s),
        "Contains: none of the 9 FDA major allergens identified"
    );
}

#[test]
fn nothing_verified_renders_only_incomplete_line() {
    let s = AllergenSummary {
        contains: vec![],
        unverified_ingredients: vec!["saffron".into()],
        view: "eu".into(),
    };
    assert_eq!(render(&s), "\nAllergen info incomplete for: saffron");
}

#[test]
fn per_item_unverified_never_claims_free() {
    let b = AllergenBlock {
        status: "unverified".into(),
        contains: vec![],
        view: "fda".into(),
    };
    let out = render_block(&b);
    assert_eq!(out, "Allergen info unverified");
    assert!(!out.contains("Contains"));
}

#[test]
fn per_item_verified_with_tags_renders_contains() {
    let b = AllergenBlock {
        status: "verified".into(),
        contains: vec![entry("gluten", Some("wheat"), "Wheat")],
        view: "fda".into(),
    };
    assert_eq!(render_block(&b), "Contains: Wheat");
}

#[test]
fn per_item_verified_empty_renders_none_identified() {
    let b = AllergenBlock {
        status: "verified".into(),
        contains: vec![],
        view: "fda".into(),
    };
    assert_eq!(
        render_block(&b),
        "Contains: none of the 9 FDA major allergens identified"
    );
}
