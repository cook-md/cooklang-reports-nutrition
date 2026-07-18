use cookmd_nutrition_client::{Client, ClientError};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_returns_facts_on_200() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .and(query_param("ingredient", "salmon"))
        .and(query_param("amount", "150"))
        .and(query_param("unit", "g"))
        .and(query_param("prep", "cooked"))
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

    let url = server.uri();
    let facts = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.nutrition_for("salmon", 150.0, "g", "cooked")
    })
    .await
    .unwrap()
    .unwrap();

    assert!((facts.protein_g - 38.1).abs() < 0.05);
    assert!((facts.kcal - 312.0).abs() < 0.5);
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_client_hits_server_once_for_repeat_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ingredient": "salmon",
            "preparation": "cooked",
            "amount": { "value": 150, "unit": "g", "mass_g": 150 },
            "macros": { "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5, "carb_g": 0.0, "fiber_g": 0.0 }
        })))
        // With caching the second identical call must NOT reach the server.
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let client = Client::new(url).cached();
        let a = client
            .nutrition_for("salmon", 150.0, "g", "cooked")
            .unwrap();
        let b = client
            .nutrition_for("salmon", 150.0, "g", "cooked")
            .unwrap();
        assert_eq!(a, b);
    })
    .await
    .unwrap();
    // Mock's `.expect(1)` is verified on server drop.
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_maps_404_ingredient_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "type": "about:blank",
            "title": "Not Found",
            "status": 404,
            "code": "ingredient_not_found",
            "detail": "ingredient not found: banana"
        })))
        .mount(&server)
        .await;

    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.nutrition_for("banana", 100.0, "g", "raw")
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(err, ClientError::NotFound(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_maps_422_unsupported_unit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "type": "about:blank",
            "title": "Unprocessable Content",
            "status": 422,
            "code": "unknown_unit",
            "detail": "unknown unit: cup"
        })))
        .mount(&server)
        .await;

    // The server emits unknown_unit; the client must map it to UnsupportedUnit.
    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.nutrition_for("salmon", 1.0, "g", "cooked")
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(err, ClientError::UnsupportedUnit(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_maps_422_density_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "type": "about:blank",
            "title": "Unprocessable Content",
            "status": 422,
            "code": "density_unavailable",
            "detail": "no density for olive oil in cup"
        })))
        .mount(&server)
        .await;

    // /nutrition can return density_unavailable; it must map to UnsupportedUnit,
    // not fall through to an opaque Server(422).
    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.nutrition_for("olive oil", 1.0, "cup", "raw")
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(err, ClientError::UnsupportedUnit(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn nutrition_for_returns_server_error_for_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nutrition"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.nutrition_for("salmon", 150.0, "g", "cooked")
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(err, ClientError::Server(500)));
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_returns_response_on_200() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "ingredient": "salmon",
                "preparation": "cooked",
                "amount": { "value": 150.0, "unit": "g", "mass_g": 150.0 },
                "macros": {
                    "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                    "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0
                },
                "micros": {},
                "vitamins": {},
                "source": "usda",
                "confidence": "confirmed",
                "warnings": []
            }],
            "failures": [],
            "totals": {
                "mass_g": 150.0,
                "macros": {
                    "kcal": 312.0, "protein_g": 38.1, "fat_g": 16.5,
                    "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0
                },
                "micros": {},
                "vitamins": {},
                "confidence": "confirmed",
                "is_partial": false,
                "included_count": 1,
                "failed_count": 0
            },
            "confidence_breakdown": {
                "confirmed_items": 1,
                "partial_items": 0,
                "estimated_items": 0,
                "estimated_ingredients": [],
                "estimated_share_of_micronutrients": null
            }
        })))
        .mount(&server)
        .await;

    let url = server.uri();
    let resp = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.aggregate(
            &[cookmd_nutrition_client::AggregateItem {
                ingredient: "salmon".into(),
                amount: 150.0,
                unit: "g".into(),
                prep: "cooked".into(),
                region: "us".into(),
            }],
            &[],
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();

    assert!((resp.totals.macros.kcal - 312.0).abs() < 0.5);
    assert_eq!(resp.totals.confidence, "confirmed");
    assert_eq!(resp.failures.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_surfaces_partial_failures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "failures": [{
                "index": 0,
                "ingredient": "unicorn",
                "error": { "code": "ingredient_not_found", "message": "ingredient not found: unicorn" }
            }],
            "totals": {
                "mass_g": 0.0,
                "macros": { "kcal": 0.0, "protein_g": 0.0, "fat_g": 0.0,
                            "carb_g": 0.0, "fiber_g": 0.0, "sugar_g": 0.0, "sat_fat_g": 0.0 },
                "micros": {},
                "vitamins": {},
                "confidence": "estimated",
                "is_partial": true,
                "included_count": 0,
                "failed_count": 1
            },
            "confidence_breakdown": {
                "confirmed_items": 0, "partial_items": 0, "estimated_items": 0,
                "estimated_ingredients": [], "estimated_share_of_micronutrients": null
            }
        })))
        .mount(&server)
        .await;

    let url = server.uri();
    let resp = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.aggregate(
            &[cookmd_nutrition_client::AggregateItem {
                ingredient: "unicorn".into(),
                amount: 1.0,
                unit: "g".into(),
                prep: "raw".into(),
                region: "us".into(),
            }],
            &[],
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();

    assert_eq!(resp.failures.len(), 1);
    assert_eq!(resp.failures[0].error.code, "ingredient_not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_returns_server_error_for_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aggregate"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.aggregate(
            &[cookmd_nutrition_client::AggregateItem {
                ingredient: "salmon".into(),
                amount: 150.0,
                unit: "g".into(),
                prep: "raw".into(),
                region: "us".into(),
            }],
            &[],
            None,
        )
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(
        err,
        cookmd_nutrition_client::ClientError::Server(500)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregate_returns_transport_error_on_connection_refused() {
    let err = tokio::task::spawn_blocking(|| {
        let client = Client::new("http://127.0.0.1:1");
        client.aggregate(
            &[cookmd_nutrition_client::AggregateItem {
                ingredient: "salmon".into(),
                amount: 150.0,
                unit: "g".into(),
                prep: "raw".into(),
                region: "us".into(),
            }],
            &[],
            None,
        )
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(
        err,
        cookmd_nutrition_client::ClientError::Transport(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn reference_intakes_memoises_after_first_fetch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "fda": { "sodium_mg": 2300.0, "fiber_g": 28.0 }
        })))
        // The second identical call must be served from the in-memory memo and
        // NOT reach the server.
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        let a = client.reference_intakes("fda").unwrap();
        let b = client.reference_intakes("fda").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.get("sodium_mg"), Some(&2300.0));
    })
    .await
    .unwrap();
    // Mock's `.expect(1)` is verified on server drop.
}

#[tokio::test(flavor = "multi_thread")]
async fn reference_intakes_errors_when_body_omits_standard() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/reference-intakes"))
        .and(query_param("standard", "fda"))
        // 200 OK but the requested slug is absent — a server-contract violation.
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "eu": { "sodium_mg": 2000.0 }
        })))
        .mount(&server)
        .await;

    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        let client = Client::new(url);
        client.reference_intakes("fda")
    })
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(err, ClientError::Server(200)));
}
