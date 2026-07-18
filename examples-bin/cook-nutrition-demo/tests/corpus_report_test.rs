//! Verifies `corpus-report.json.jinja` emits one JSON row whose `failures[]`
//! carry the failure `code` and the originating `unit`, correlated by index.
//! Uses a wiremock; no real DB or service.

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

/// One resolved (salmon) + one density failure (star fruit at index 1). The
/// sample recipe sends star fruit as a bare count `{1}`, so the echoed unit is
/// empty; the point of the test is that `code` is present and the row parses.
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
            "error": { "code": "density_unavailable", "message": "no density" }
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
async fn report_row_carries_code_and_unit() {
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
            .arg(fixture("corpus-report.json.jinja"))
            .arg(fixture("corpus-eval-sample.cook"))
            .output()
            .expect("run demo")
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let row: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout not valid JSON ({e}):\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });

    assert_eq!(row["ingredients_resolved"], 1, "row: {row}");
    assert_eq!(row["ingredients_failed"], 1, "row: {row}");
    assert_eq!(
        row["failures"][0]["code"], "density_unavailable",
        "row: {row}"
    );
    assert_eq!(row["failures"][0]["ingredient"], "star fruit", "row: {row}");
    // unit echoes what we sent; the fixture sends star fruit as a bare count
    // `{1}`, so the round-tripped unit is the empty string (key always present).
    assert_eq!(row["failures"][0]["unit"], "", "row: {row}");
    let listed = row["ingredients_listed"].as_i64().expect("listed int");
    assert!(listed >= 2, "row: {row}");
}
