//! Verifies `corpus-eval.json.jinja` emits one valid JSON row with correct
//! resolution counts, using a wiremock that returns one resolved item and one
//! soft failure. No real DB or service required.

use std::path::PathBuf;
use std::process::Command;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_cook-nutrition-demo")
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
        .display()
        .to_string()
}

/// One resolved item (salmon) + one soft failure (star fruit). Numbers are
/// arbitrary but distinct so the assertions are meaningful.
fn aggregate_response() -> serde_json::Value {
    json!({
        "items": [{
            "ingredient": "salmon", "preparation": "cooked",
            "amount": { "value": 150.0, "unit": "g", "mass_g": 150.0 },
            "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                        "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
            "micros": {}, "vitamins": {}, "source": "usda",
            "confidence": "confirmed", "warnings": []
        }],
        "failures": [{
            "index": 1, "ingredient": "star fruit",
            "error": { "code": "ingredient_not_found", "message": "no match" }
        }],
        "totals": {
            "mass_g": 150.0,
            "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                        "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
            "micros": {}, "vitamins": {},
            "confidence": "confirmed", "confidence_weighted": "confirmed",
            "is_partial": true, "included_count": 1, "failed_count": 1
        },
        "confidence_breakdown": {
            "confirmed_items": 1, "partial_items": 0, "estimated_items": 0,
            "estimated_ingredients": [], "estimated_share_of_micronutrients": null
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn corpus_eval_emits_valid_json_row() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(aggregate_response()))
        .mount(&server)
        .await;
    let server_uri = server.uri();

    let out = tokio::task::spawn_blocking(move || {
        Command::new(binary_path())
            .env("NUTRITION_API_URL", &server_uri)
            .arg(fixture("corpus-eval.json.jinja"))
            .arg(fixture("corpus-eval-sample.cook"))
            .output()
            .expect("run demo")
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let row: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout was not valid JSON ({e}):\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });

    assert_eq!(row["ingredients_resolved"], 1, "row: {row}");
    assert_eq!(row["ingredients_failed"], 1, "row: {row}");
    assert_eq!(row["unresolved"], json!(["star fruit"]), "row: {row}");
    assert_eq!(row["total"]["kcal"], 312.0, "row: {row}");
    assert_eq!(
        row["confidence_breakdown"]["confirmed_items"], 1,
        "row: {row}"
    );
    assert!(row["servings"].is_null(), "row: {row}");

    // cooklang-reports' get_ingredient_list counts the unquantified `serving platter{}`
    // too, so we assert a floor + the listed>=resolved+failed invariant rather than
    // hard-coupling to cooklang's exact count of an implementation detail.
    let listed = row["ingredients_listed"].as_i64().expect("listed is int");
    assert!(listed >= 2, "expected >=2 listed, row: {row}");
    assert!(
        listed
            >= row["ingredients_resolved"].as_i64().unwrap()
                + row["ingredients_failed"].as_i64().unwrap(),
        "expected listed >= resolved + failed, row: {row}"
    );
}

/// Empty items, one soft failure whose ingredient name contains both a backslash
/// and a double-quote. Same totals/breakdown shape as `aggregate_response()` but
/// zeroed out, so the only meaningful output is the manually-escaped `unresolved`.
fn aggregate_response_with_special_chars(ingredient: &str) -> serde_json::Value {
    json!({
        "items": [],
        "failures": [{
            "index": 0, "ingredient": ingredient,
            "error": { "code": "ingredient_not_found", "message": "no match" }
        }],
        "totals": {
            "mass_g": 0.0,
            "macros": { "kcal": 0.0, "protein_g": 0.0, "fat_g": 0.0,
                        "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
            "micros": {}, "vitamins": {},
            "confidence": "confirmed", "confidence_weighted": "confirmed",
            "is_partial": true, "included_count": 0, "failed_count": 1
        },
        "confidence_breakdown": {
            "confirmed_items": 0, "partial_items": 0, "estimated_items": 0,
            "estimated_ingredients": [], "estimated_share_of_micronutrients": null
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn corpus_eval_escapes_special_chars_in_unresolved() {
    // Actual string value: 5\" pan "cast" iron  (one backslash, two double-quotes).
    let nasty = "5\\\" pan \"cast\" iron";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(aggregate_response_with_special_chars(nasty)),
        )
        .mount(&server)
        .await;
    let server_uri = server.uri();

    let out = tokio::task::spawn_blocking(move || {
        Command::new(binary_path())
            .env("NUTRITION_API_URL", &server_uri)
            .arg(fixture("corpus-eval.json.jinja"))
            .arg(fixture("corpus-eval-sample.cook"))
            .output()
            .expect("run demo")
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The key check: if the manual quote/backslash escaping were wrong, the row
    // would be malformed JSON and this parse would fail.
    let row: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout was not valid JSON ({e}):\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });

    assert_eq!(
        row["unresolved"][0],
        json!("5\\\" pan \"cast\" iron"),
        "round-tripped name must match original, row: {row}"
    );
}
