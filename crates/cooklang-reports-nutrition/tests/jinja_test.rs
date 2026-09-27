use cooklang_reports_nutrition::compare_fn;
use minijinja::Environment;

#[test]
fn compare_gte_true() {
    let mut env = Environment::new();
    env.add_function("compare", compare_fn);
    env.add_template("t", "{{ compare(39, 30, 'gte') }}")
        .unwrap();
    let out = env.get_template("t").unwrap().render(()).unwrap();
    assert_eq!(out, "true");
}

#[test]
fn compare_gte_false() {
    let mut env = Environment::new();
    env.add_function("compare", compare_fn);
    env.add_template("t", "{{ compare(10, 30, 'gte') }}")
        .unwrap();
    let out = env.get_template("t").unwrap().render(()).unwrap();
    assert_eq!(out, "false");
}

#[test]
fn compare_lte_true() {
    let mut env = Environment::new();
    env.add_function("compare", compare_fn);
    env.add_template("t", "{{ compare(10, 30, 'lte') }}")
        .unwrap();
    let out = env.get_template("t").unwrap().render(()).unwrap();
    assert_eq!(out, "true");
}

#[test]
fn compare_eq_true() {
    let mut env = Environment::new();
    env.add_function("compare", compare_fn);
    env.add_template("t", "{{ compare(30, 30, 'eq') }}")
        .unwrap();
    let out = env.get_template("t").unwrap().render(()).unwrap();
    assert_eq!(out, "true");
}

#[test]
fn compare_unknown_op_is_false() {
    let mut env = Environment::new();
    env.add_function("compare", compare_fn);
    env.add_template("t", "{{ compare(30, 30, 'within') }}")
        .unwrap();
    let out = env.get_template("t").unwrap().render(()).unwrap();
    assert_eq!(out, "false");
}

use cooklang_reports_nutrition::within_tol;

#[test]
fn within_tol_exact_match() {
    assert!(within_tol(100.0, 100.0, 10.0));
}

#[test]
fn within_tol_at_upper_bound() {
    assert!(within_tol(110.0, 100.0, 10.0));
}

#[test]
fn within_tol_just_over_bound() {
    assert!(!within_tol(110.01, 100.0, 10.0));
}

#[test]
fn within_tol_at_lower_bound() {
    assert!(within_tol(90.0, 100.0, 10.0));
}

#[test]
fn within_tol_zero_base_exact_zero_passes() {
    assert!(within_tol(0.0, 0.0, 10.0));
}

#[test]
fn within_tol_zero_base_nonzero_actual_fails() {
    assert!(!within_tol(0.1, 0.0, 10.0));
}

#[test]
fn within_tol_negative_tol_fails() {
    assert!(!within_tol(100.0, 100.0, -1.0));
}

#[test]
fn within_tol_nan_fails() {
    assert!(!within_tol(f64::NAN, 100.0, 10.0));
}

use std::sync::Arc;

use cooklang_reports::{Config, render_template_with_config};
use cooklang_reports_nutrition::NutritionExtension;
use cooklang_reports_nutrition::checks::CheckTracker;
use cookmd_nutrition_client::Client;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn record_check_records_into_tracker() {
    use cooklang_reports::{Config, render_template_with_config};

    let client = Arc::new(Client::new("http://127.0.0.1:1".to_string()));
    let ext = NutritionExtension::new(client);
    let tracker = ext.checks();
    let tmpl = "{{ record_check('protein OK', true) }}{{ record_check('fat too high', false) }}";
    let config = Config::builder()
        .base_path(std::path::Path::new("."))
        .build()
        .with_extension(ext);
    let _ = render_template_with_config("", tmpl, &config).unwrap();
    let snap = tracker.snapshot();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].label, "protein OK");
    assert!(snap[0].ok);
    assert_eq!(snap[1].label, "fat too high");
    assert!(!snap[1].ok);
    assert_eq!(tracker.failed_count(), 1);
}

#[test]
fn failed_checks_returns_only_failed_records() {
    use cooklang_reports::{Config, render_template_with_config};

    let client = Arc::new(Client::new("http://127.0.0.1:1".to_string()));
    let ext = NutritionExtension::new(client);
    let tmpl = r#"{{ record_check('a', true) }}{{ record_check('b', false) }}{% for c in failed_checks() %}{{ c.label }};{% endfor %}"#;
    let config = Config::builder()
        .base_path(std::path::Path::new("."))
        .build()
        .with_extension(ext);
    let out = render_template_with_config("", tmpl, &config).unwrap();
    assert!(
        out.contains("b;"),
        "expected only failed labels, got: {out}"
    );
    assert!(
        !out.contains("a;"),
        "passed labels must not appear, got: {out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_in_template_returns_protein() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ingredient": "salmon",
            "preparation": "cooked",
            "amount": { "value": 150, "unit": "g", "mass_g": 150 },
            "macros": {
                "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                "carb_g": 0.0, "fiber_g": 0.0
            }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);

        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "{% for ingredient in ingredients %}\
{%- set n = nutrition_for(ingredient) -%}\
P={{ n.protein_g }}\
{% endfor %}";

        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("P=38.1"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_passes_unit_to_service() {
    let server = MockServer::start().await;
    // Expect unit=cup in the query string
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .and(query_param("unit", "cup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ingredient": "milk",
            "preparation": "raw",
            "amount": { "value": 1.0, "unit": "cup", "mass_g": 244.0 },
            "macros": {
                "kcal": 149.0, "protein_g": 7.7, "fat_g": 8.0,
                "carb_g": 11.7, "fiber_g": 0.0
            }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Add @milk{1%cup} to the mix.";
        let template = "{% for ingredient in ingredients %}{%- set n = nutrition_for(ingredient) -%}K={{ n.kcal }}{% endfor %}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("K=149"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_nutrition_returns_totals() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "salmon", "preparation": "cooked",
                "amount": { "value": 150.0, "unit": "g", "mass_g": 150.0 },
                "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {}, "source": "usda",
                "confidence": "confirmed", "warnings": []
            }],
            "failures": [],
            "totals": {
                "mass_g": 150.0,
                "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {},
                "confidence": "confirmed", "is_partial": false,
                "included_count": 1, "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
                "estimated_ingredients": [], "estimated_share_of_micronutrients": null
            }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "{% set agg = aggregate_nutrition(ingredients) %}K={{ agg.totals.macros.kcal }}|C={{ agg.totals.confidence }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("K=312"), "got: {rendered}");
    assert!(rendered.contains("C=confirmed"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_nutrition_forwards_optional_standard() {
    let server = MockServer::start().await;
    // Only matches when the request body carries `"reference": "eu"`; an
    // unmatched request gets wiremock's 404 and the render fails.
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .and(body_partial_json(serde_json::json!({ "reference": "eu" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "butter", "preparation": "raw",
                "amount": { "value": 50.0, "unit": "g", "mass_g": 50.0 },
                "macros": { "kcal": 358.0, "protein_g": 0.4, "fat_g": 40.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 25.7 },
                "micros": {}, "vitamins": {}, "source": "usda",
                "confidence": "confirmed", "warnings": [],
                "allergens": { "status": "verified",
                               "contains": [{ "class": "milk", "label": "Milk" }], "view": "eu" }
            }],
            "failures": [],
            "totals": {
                "mass_g": 50.0,
                "macros": { "kcal": 358.0, "protein_g": 0.4, "fat_g": 40.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 25.7 },
                "micros": {}, "vitamins": {},
                "confidence": "confirmed", "is_partial": false,
                "included_count": 1, "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
                "estimated_ingredients": [], "estimated_share_of_micronutrients": null
            },
            "allergen_summary": { "contains": [{ "class": "milk", "label": "Milk" }],
                                  "unverified_ingredients": [], "view": "eu" }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Melt @butter{50%g}.";
        let template = r#"{% set agg = aggregate_nutrition(ingredients, "eu") %}{{ agg["items"][0].allergens.status }}|{{ agg["items"][0].allergens.contains[0].class }}"#;
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert_eq!(rendered, "verified|milk");
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_nutrition_skips_non_numeric_quantities() {
    let server = MockServer::start().await;
    // If a non-numeric ingredient were sent the mock wouldn't match and would panic.
    // We assert the call goes through with only the numeric ingredient.
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "salmon", "preparation": "raw",
                "amount": { "value": 150.0, "unit": "g", "mass_g": 150.0 },
                "macros": { "kcal": 200.0, "protein_g": 20.0, "fat_g": 10.0,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {}, "source": "usda",
                "confidence": "confirmed", "warnings": []
            }],
            "failures": [],
            "totals": {
                "mass_g": 150.0,
                "macros": { "kcal": 200.0, "protein_g": 20.0, "fat_g": 10.0,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {},
                "confidence": "confirmed", "is_partial": false,
                "included_count": 1, "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
                "estimated_ingredients": [], "estimated_share_of_micronutrients": null
            }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        // "some" is a non-numeric quantity — should be skipped silently
        let recipe = "Add @salt{some} to taste. Pan-fry @salmon{150%g}.";
        let template =
            "{% set agg = aggregate_nutrition(ingredients) %}K={{ agg.totals.macros.kcal }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("K=200"), "got: {rendered}");
}

fn aggregate_mock_response() -> serde_json::Value {
    serde_json::json!({
        "items": [],
        "failures": [],
        "totals": {
            "mass_g": 150.0,
            "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                        "carb_g": 1.0, "fiber_g": 0.5, "sugar_g": 0.2, "sat_fat_g": 3.0 },
            "micros": { "iron_mg": 2.5 },
            "vitamins": {},
            "confidence": "confirmed",
            "is_partial": false,
            "included_count": 1,
            "failed_count": 0
        },
        "confidence_breakdown": {
            "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
            "estimated_ingredients": [], "estimated_share_of_micronutrients": null
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn macros_returns_macro_object() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(aggregate_mock_response()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "{% set m = macros(ingredients) %}P={{ m.protein_g }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("P=38.1"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn total_calories_returns_kcal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(aggregate_mock_response()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "K={{ total_calories(ingredients) }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("K=312"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrient_total_returns_value_for_present_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(aggregate_mock_response()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "Fe={{ nutrient_total(ingredients, 'iron_mg') }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("Fe=2.5"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrient_total_returns_zero_for_absent_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(aggregate_mock_response()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "Zn={{ nutrient_total(ingredients, 'zinc_mg') }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("Zn=0"), "got: {rendered}");
}

fn render_ck(tmpl: &str) -> (String, CheckTracker) {
    use cooklang_reports::{Config, render_template_with_config};
    let client = Arc::new(Client::new("http://127.0.0.1:1".to_string()));
    let ext = NutritionExtension::new(client);
    let tracker = ext.checks();
    let config = Config::builder()
        .base_path(std::path::Path::new("."))
        .build()
        .with_extension(ext);
    let out = render_template_with_config("", tmpl, &config).unwrap();
    (out, tracker)
}

#[test]
fn ck_min_records_pass_when_value_meets_min() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.min(80, {"min": 75}, "protein_g") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].ok);
}

#[test]
fn ck_min_records_fail_when_value_below_min() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.min(50, {"min": 75}, "protein_g") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(!snap[0].ok);
}

#[test]
fn ck_min_silently_skips_when_target_has_no_min() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.min(50, {"max": 100}, "x") }}"#;
    let (_, t) = render_ck(tmpl);
    assert_eq!(t.snapshot().len(), 0);
}

#[test]
fn ck_range_records_one_composite_check() {
    let tmpl =
        r#"{% import "ck" as ck %}{{ ck.range(2000, {"min": 1800, "max": 2200}, "energy") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].ok);
    assert!(snap[0].label.contains("[1800, 2200]"));
}

#[test]
fn ck_within_uses_within_tol() {
    let tmpl =
        r#"{% import "ck" as ck %}{{ ck.within(105, {"max": 100, "tol_pct": 10}, "sat_fat") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].ok);
}

#[test]
fn ck_within_fails_closed_when_tol_pct_missing() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.within(100, {"max": 100}, "sat_fat") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(!snap[0].ok);
    assert!(snap[0].label.contains("tol_pct"));
}

#[test]
fn ck_between_uses_free_bounds() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.between(50, 25, 100, "fiber") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].ok);
}

#[test]
fn ck_absent_passes_when_name_not_in_matched() {
    let tmpl = r#"{% import "ck" as ck %}{{ ck.absent("peanut") }}"#;
    let (_, t) = render_ck(tmpl);
    let snap = t.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn ck_absent_finds_match_through_aggregate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "failures": [],
            "totals": {
                "mass_g": 0,
                "macros": {"kcal":0,"protein_g":0,"fat_g":0,"carb_g":0,"fiber_g":0,"sugar_g":0,"sat_fat_g":0},
                "micros": {},
                "vitamins": {},
                "confidence": "confirmed",
                "is_partial": false,
                "included_count": 0,
                "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items":0,"partial_items":0,"estimated_items":0,
                "estimated_ingredients": [],
                "estimated_share_of_micronutrients": null
            },
            "matched_exclusions": ["peanut"],
            "unresolved_exclusions": []
        })))
        .mount(&server)
        .await;
    let base = server.uri();

    let snapshot = tokio::task::spawn_blocking(move || {
        use cooklang_reports::{Config, render_template_with_config};
        let client = Arc::new(Client::new(base));
        let profile = cooklang_reports_nutrition::client::ClientProfile {
            name: "T".into(),
            exclusions: vec!["peanut".into()],
            targets: Default::default(),
        };
        let ext = NutritionExtension::new(client).with_client(profile);
        let tracker = ext.checks();
        let tmpl = r#"{% import "ck" as ck %}{{ macros([{"name":"peanut","quantity":{"value":10,"unit":"g"}}]) }}{{ ck.absent("peanut") }}"#;
        let config = Config::builder()
            .base_path(std::path::Path::new("."))
            .build()
            .with_extension(ext);
        let _ = render_template_with_config("", tmpl, &config).unwrap();
        tracker.snapshot()
    })
    .await
    .unwrap();

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].label, "absent: peanut");
    assert!(
        !snapshot[0].ok,
        "peanut should be present, check should fail"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unresolved_exclusion_surfaces_to_macro_and_fn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "failures": [],
            "totals": {
                "mass_g": 0,
                "macros": {"kcal":0,"protein_g":0,"fat_g":0,"carb_g":0,"fiber_g":0,"sugar_g":0,"sat_fat_g":0},
                "micros": {},
                "vitamins": {},
                "confidence": "confirmed",
                "is_partial": false,
                "included_count": 0,
                "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items":0,"partial_items":0,"estimated_items":0,
                "estimated_ingredients": [],
                "estimated_share_of_micronutrients": null
            },
            "matched_exclusions": [],
            "unresolved_exclusions": ["mystery"]
        })))
        .mount(&server)
        .await;
    let base = server.uri();

    let snapshot = tokio::task::spawn_blocking(move || {
        use cooklang_reports::{Config, render_template_with_config};
        let client = Arc::new(Client::new(base));
        let profile = cooklang_reports_nutrition::client::ClientProfile {
            name: "T".into(),
            exclusions: vec!["mystery".into()],
            targets: Default::default(),
        };
        let ext = NutritionExtension::new(client).with_client(profile);
        let tracker = ext.checks();
        let tmpl = r#"{% import "ck" as ck %}{{ macros([{"name":"x","quantity":{"value":10,"unit":"g"}}]) }}{{ ck.absent("mystery") }}"#;
        let config = Config::builder()
            .base_path(std::path::Path::new("."))
            .build()
            .with_extension(ext);
        let _ = render_template_with_config("", tmpl, &config).unwrap();
        tracker.snapshot()
    })
    .await
    .unwrap();

    assert_eq!(snapshot.len(), 1);
    assert!(
        snapshot[0].label.contains("[unresolved]"),
        "label was: {}",
        snapshot[0].label
    );
    assert!(!snapshot[0].ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn matched_exclusions_unions_across_aggregate_calls() {
    use wiremock::matchers::body_string_contains;
    let server = MockServer::start().await;
    let resp_template = |matched: &[&str]| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "failures": [],
            "totals": {
                "mass_g": 0,
                "macros": {"kcal":0,"protein_g":0,"fat_g":0,"carb_g":0,"fiber_g":0,"sugar_g":0,"sat_fat_g":0},
                "micros": {},
                "vitamins": {},
                "confidence": "confirmed",
                "is_partial": false,
                "included_count": 0,
                "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items":0,"partial_items":0,"estimated_items":0,
                "estimated_ingredients": [],
                "estimated_share_of_micronutrients": null
            },
            "matched_exclusions": matched,
            "unresolved_exclusions": []
        }))
    };
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .and(body_string_contains("apple"))
        .respond_with(resp_template(&["peanut"]))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .and(body_string_contains("banana"))
        .respond_with(resp_template(&["shellfish"]))
        .mount(&server)
        .await;
    let base = server.uri();

    let output = tokio::task::spawn_blocking(move || {
        use cooklang_reports::{Config, render_template_with_config};
        let client = Arc::new(Client::new(base));
        let profile = cooklang_reports_nutrition::client::ClientProfile {
            name: "T".into(),
            exclusions: vec!["peanut".into(), "shellfish".into()],
            targets: Default::default(),
        };
        let ext = NutritionExtension::new(client).with_client(profile);
        let tmpl = r#"{{ macros([{"name":"apple","quantity":{"value":10,"unit":"g"}}]) }}{{ macros([{"name":"banana","quantity":{"value":10,"unit":"g"}}]) }}MATCHED:{% for x in matched_exclusions() %}{{ x }};{% endfor %}"#;
        let config = Config::builder()
            .base_path(std::path::Path::new("."))
            .build()
            .with_extension(ext);
        render_template_with_config("", tmpl, &config).unwrap()
    })
    .await
    .unwrap();

    // BTreeSet iteration is sorted: peanut < shellfish.
    assert!(
        output.contains("MATCHED:peanut;shellfish;"),
        "output was: {output}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn vitamins_returns_vitamin_object() {
    let server = MockServer::start().await;
    let mut body = aggregate_mock_response();
    body["totals"]["vitamins"] = serde_json::json!({ "vit_d_iu": 750.0 });
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Pan-fry @salmon{150%g}(cooked) and serve.";
        let template = "{% set v = vitamins(ingredients) %}D={{ v.vit_d_iu }}";
        render_template_with_config(recipe, template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("D=750"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_amount_returns_full_item() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "salmon", "preparation": "cooked",
                "amount": { "value": 150.0, "unit": "g", "mass_g": 150.0 },
                "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {}, "source": "usda",
                "confidence": "confirmed", "warnings": []
            }],
            "failures": [],
            "totals": {
                "mass_g": 150.0,
                "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {}, "vitamins": {},
                "confidence": "confirmed", "is_partial": false,
                "included_count": 1, "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
                "estimated_ingredients": [], "estimated_share_of_micronutrients": null
            }
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let template =
            "{% set n = nutrition_for_amount('salmon', 150, 'g', 'cooked') %}P={{ n.macros.protein_g }}|C={{ n.confidence }}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("P=38.1"), "got: {rendered}");
    assert!(rendered.contains("C=confirmed"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn is_in_category_true_for_member() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/categories/oily_fish/check"))
        .and(query_param("ingredient", "salmon"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "in_category": true
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let template = "R={{ is_in_category('salmon', 'oily_fish') }}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("R=true"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn category_servings_counts_meals_with_member() {
    let server = MockServer::start().await;
    // salmon is in the category, oats are not.
    Mock::given(method("GET"))
        .and(path("/categories/omega3_source/check"))
        .and(query_param("ingredient", "salmon"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"in_category": true})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/categories/omega3_source/check"))
        .and(query_param("ingredient", "oats"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"in_category": false})),
        )
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        // Two meals: breakfast (oats only → no), dinner (salmon → yes) ⇒ 1 serving.
        let template = r#"{% set plan = {"days":[{"meals":[
            {"recipes":[{"ingredients":[{"name":"oats"}]}]},
            {"recipes":[{"ingredients":[{"name":"salmon"}]}]}
        ]}]} %}S={{ category_servings(plan, "omega3_source") }}"#;
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("S=1"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_returns_numeric_result() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/convert"))
        .and(query_param("from", "oz"))
        .and(query_param("to", "g"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ingredient": null, "from": "oz", "to": "g",
            "value": 2.0, "result": 56.699, "mass_g": 56.699
        })))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let template = "G={{ convert(2, 'oz', 'g') }}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("G=56.699"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ck_absent_works_with_nutrition_for_when_profile_has_exclusions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "peanut",
                "preparation": "raw",
                "amount": {"value": 10.0, "unit": "g", "mass_g": 10.0},
                "macros": {"kcal": 56.7, "protein_g": 2.6, "fat_g": 4.9, "carb_g": 1.6, "fiber_g": 0.8, "sugar_g": 0.0, "sat_fat_g": 0.7},
                "micros": {},
                "vitamins": {},
                "source": "test",
                "confidence": "confirmed",
                "warnings": [],
                "matched_exclusions": ["peanut"]
            }],
            "failures": [],
            "totals": {
                "mass_g": 10,
                "macros": {"kcal":56.7,"protein_g":2.6,"fat_g":4.9,"carb_g":1.6,"fiber_g":0.8,"sugar_g":0.0,"sat_fat_g":0.7},
                "micros": {},
                "vitamins": {},
                "confidence": "confirmed",
                "is_partial": false,
                "included_count": 1,
                "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items":1,"partial_items":0,"estimated_items":0,
                "estimated_ingredients": [],
                "estimated_share_of_micronutrients": null
            },
            "matched_exclusions": ["peanut"],
            "unresolved_exclusions": []
        })))
        .mount(&server)
        .await;
    let base = server.uri();

    let snapshot = tokio::task::spawn_blocking(move || {
        use cooklang_reports::{Config, render_template_with_config};
        let client = Arc::new(Client::new(base));
        let profile = cooklang_reports_nutrition::client::ClientProfile {
            name: "T".into(),
            exclusions: vec!["peanut".into()],
            targets: Default::default(),
        };
        let ext = NutritionExtension::new(client).with_client(profile);
        let tracker = ext.checks();
        // Iterate ingredients with nutrition_for — would have silently passed ck.absent before this fix.
        let tmpl = r#"{% import "ck" as ck %}{% for ing in ingredients %}{{ nutrition_for(ing) }}{% endfor %}{{ ck.absent("peanut") }}"#;
        let recipe = "@peanut{10%g}";
        let config = Config::builder()
            .base_path(std::path::Path::new("."))
            .build()
            .with_extension(ext);
        let _ = render_template_with_config(recipe, tmpl, &config).unwrap();
        tracker.snapshot()
    })
    .await
    .unwrap();

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].label, "absent: peanut");
    assert!(
        !snapshot[0].ok,
        "peanut should be detected via nutrition_for, check should fail"
    );
}

// ---------------------------------------------------------------------------
// reference_intake / dv_percent
// ---------------------------------------------------------------------------

fn reference_intakes_mock_body() -> serde_json::Value {
    serde_json::json!({ "fda": { "sodium_mg": 2300.0 } })
}

#[tokio::test(flavor = "multi_thread")]
async fn reference_intake_returns_value_for_known_nutrient() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reference_intakes_mock_body()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let template = "V={{ reference_intake('sodium_mg') }}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("V=2300"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reference_intake_returns_undefined_for_unknown_nutrient() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reference_intakes_mock_body()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        // `is undefined` is the correct minijinja test for Value::UNDEFINED
        let template =
            "{% if reference_intake('no_such') is undefined %}YES{% else %}NO{% endif %}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert_eq!(rendered.trim(), "YES", "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dv_percent_returns_percentage_for_known_nutrient() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reference_intakes_mock_body()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        // 230 / 2300 * 100 = 10.0
        let template = "P={{ dv_percent('sodium_mg', 230) }}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert!(rendered.contains("P=10"), "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dv_percent_returns_undefined_for_unknown_nutrient() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reference_intakes_mock_body()))
        .mount(&server)
        .await;

    let base = server.uri();
    let rendered = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let config = Config::builder().build().with_extension(ext);
        let template = "{% if dv_percent('no_such', 100) is undefined %}YES{% else %}NO{% endif %}";
        render_template_with_config("", template, &config).unwrap()
    })
    .await
    .unwrap();

    assert_eq!(rendered.trim(), "YES", "got: {rendered}");
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_failures_are_recorded_out_of_band() {
    let server = MockServer::start().await;
    let mut body = aggregate_mock_response();
    body["failures"] = serde_json::json!([{
        "index": 0,
        "ingredient": "unobtainium",
        "error": {
            "code": "ingredient_not_found",
            "message": "no match for 'unobtainium'",
            "suggestions": ["onion"]
        }
    }]);
    body["totals"]["is_partial"] = serde_json::json!(true);
    body["totals"]["failed_count"] = serde_json::json!(1);
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let base = server.uri();
    let (rendered, snap) = tokio::task::spawn_blocking(move || {
        let client = Arc::new(Client::new(base));
        let ext = NutritionExtension::new(client);
        let failures = ext.failures();
        let config = Config::builder().build().with_extension(ext);
        let recipe = "Stir in @unobtainium{1%g}.";
        let template =
            "{% set agg = aggregate_nutrition(ingredients) %}F={{ agg.totals.failed_count }}";
        let rendered = render_template_with_config(recipe, template, &config).unwrap();
        (rendered, failures.snapshot())
    })
    .await
    .unwrap();

    assert!(rendered.contains("F=1"), "got: {rendered}");
    assert_eq!(snap.len(), 1, "expected one recorded failure: {snap:?}");
    assert_eq!(snap[0]["ingredient"], "unobtainium");
    assert_eq!(snap[0]["error"]["code"], "ingredient_not_found");
    assert_eq!(snap[0]["error"]["suggestions"][0], "onion");
}
