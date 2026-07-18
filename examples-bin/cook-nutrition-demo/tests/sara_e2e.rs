//! End-to-end tests for the Sara Mendez reference dietitian templates.
//!
//! Spawns the `cook-nutrition-demo` binary against a wiremock that stands in
//! for the nutrition service, then asserts the rendered output and exit code.
//! No real DB or service is required.

use std::path::PathBuf;
use std::process::Command;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_cook-nutrition-demo")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn fixture(name: &str) -> String {
    fixtures_dir().join(name).display().to_string()
}

/// A successful aggregate response that satisfies *all* of Sara's recipe-level
/// targets:
///
/// - kcal=2000  → in [1800, 2200]
/// - protein_g=100 → ≥ 75
/// - sat_fat_g=20.0 → within 10% of 20
/// - fiber_g=30 → ≥ 25
///
/// `matched_exclusions` is empty, so the `ck.absent(...)` checks for peanut
/// and shellfish both pass.
///
/// `allergen_summary` carries one verified allergen plus one unverified
/// ingredient so the weekly report's `## Allergens` section exercises the
/// populated "Contains at least" + incomplete-line path.
fn pass_response() -> serde_json::Value {
    json!({
        "items": [],
        "failures": [],
        "totals": {
            "mass_g": 320.0,
            "macros": {
                "kcal": 2000.0,
                "protein_g": 100.0,
                "fat_g": 30.0,
                "carb_g": 200.0,
                "fiber_g": 30.0,
                "sugar_g": 10.0,
                "sat_fat_g": 20.0
            },
            "micros": {},
            "vitamins": {},
            "confidence": "confirmed",
            "confidence_weighted": "confirmed",
            "is_partial": false,
            "included_count": 4,
            "failed_count": 0
        },
        "confidence_breakdown": {
            "confirmed_items": 4,
            "partial_items": 0,
            "estimated_items": 0,
            "estimated_ingredients": [],
            "estimated_share_of_micronutrients": null
        },
        "matched_exclusions": [],
        "unresolved_exclusions": [],
        "allergen_summary": {
            "contains": [
                { "class": "milk", "label": "Milk" }
            ],
            "unverified_ingredients": ["saffron"],
            "view": "fda"
        }
    })
}

/// Same shape as `pass_response`, but with protein_g lowered below the 75 g
/// target so the `ck.min` check fails and the binary exits 1.
fn fail_response_low_protein() -> serde_json::Value {
    let mut v = pass_response();
    v["totals"]["macros"]["protein_g"] = json!(30.0);
    v
}

async fn mount_aggregate(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1..)
        .mount(server)
        .await;
}

/// Mount the taxonomy check used by `category_servings` so the weekly report's
/// omega-3 variety check can resolve. Returns `in_category: true` for every
/// ingredient so each meal counts as an omega-3 serving.
async fn mount_category_check(server: &MockServer) {
    use wiremock::matchers::path_regex;
    Mock::given(method("GET"))
        .and(path_regex(r"^/categories/.+/check$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "in_category": true })))
        .mount(server)
        .await;
}

fn run_demo_with_url(api_url: &str, template: &str, recipe: &str) -> std::process::Output {
    Command::new(binary_path())
        .env("NUTRITION_API_URL", api_url)
        .arg("--client")
        .arg(fixture("sara-mendez.yaml"))
        .arg(template)
        .arg(recipe)
        .output()
        .expect("run demo")
}

#[tokio::test(flavor = "multi_thread")]
async fn sara_recipe_pass_case_exits_zero() {
    let server = MockServer::start().await;
    mount_aggregate(&server, pass_response()).await;
    let server_uri = server.uri();

    let out = tokio::task::spawn_blocking(move || {
        run_demo_with_url(
            &server_uri,
            &fixture("sara-recipe-report.md.jinja"),
            &fixture("sara-recipe-input.cook"),
        )
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status,
    );
    assert!(
        stdout.contains("Dietitian Report"),
        "stdout missing header:\n{stdout}"
    );
    assert!(
        stdout.contains("Sara Mendez"),
        "stdout missing client name:\n{stdout}"
    );
    assert!(
        stdout.contains("All checks passed."),
        "stdout missing pass marker:\n{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sara_recipe_fail_case_exits_one() {
    let server = MockServer::start().await;
    mount_aggregate(&server, fail_response_low_protein()).await;
    let server_uri = server.uri();

    let out = tokio::task::spawn_blocking(move || {
        run_demo_with_url(
            &server_uri,
            &fixture("sara-recipe-report.md.jinja"),
            &fixture("sara-recipe-input.cook"),
        )
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("## Concerns"),
        "stdout missing Concerns section:\n{stdout}"
    );
    assert!(
        stdout.contains("protein"),
        "stdout missing failed protein label:\n{stdout}"
    );
}

#[test]
fn missing_client_yaml_exits_two() {
    // No server needed: we should bail before any HTTP traffic. Pin
    // NUTRITION_API_URL to a dead address so the test doesn't accidentally
    // succeed against a real service if the developer has one running.
    let out = Command::new(binary_path())
        .env("NUTRITION_API_URL", "http://127.0.0.1:1")
        .arg("--client")
        .arg("/nonexistent/profile.yaml")
        .arg(fixture("sara-recipe-report.md.jinja"))
        .arg(fixture("sara-recipe-input.cook"))
        .output()
        .expect("run demo");

    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for missing client, got {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sara_weekly_pass_case_exits_zero() {
    let server = MockServer::start().await;
    mount_aggregate(&server, pass_response()).await;
    mount_category_check(&server).await;
    let server_uri = server.uri();

    let out = tokio::task::spawn_blocking(move || {
        Command::new(binary_path())
            .env("NUTRITION_API_URL", &server_uri)
            .args([
                "--base-path",
                &fixtures_dir().display().to_string(),
                "--client",
                &fixture("sara-mendez.yaml"),
                &fixture("sara-week-report.md.jinja"),
                &fixture("sara-week-input.menu"),
            ])
            .output()
            .expect("run demo")
    })
    .await
    .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status,
    );
    assert!(
        stdout.contains("Weekly Dietitian Report"),
        "stdout missing weekly header:\n{stdout}"
    );
    assert!(
        stdout.contains("2026-06-15"),
        "stdout missing first day:\n{stdout}"
    );
    assert!(
        stdout.contains("2026-06-16"),
        "stdout missing second day:\n{stdout}"
    );
    assert!(
        stdout.contains("Contains at least: Milk"),
        "stdout missing allergen floor line:\n{stdout}"
    );
    assert!(
        stdout.contains("Allergen info incomplete for: saffron"),
        "stdout missing allergen incomplete line:\n{stdout}"
    );
    assert!(
        stdout.contains("All checks passed."),
        "stdout missing pass marker:\n{stdout}"
    );
}
