use cookmd_nutrition_client::{Client, ClientError};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Responds to classify with `egg` -> dairy_and_egg/egg, everything else null.
struct ClassifyResponder;
impl Respond for ClassifyResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let mut results = serde_json::Map::new();
        for n in body["ingredients"].as_array().unwrap() {
            let n = n.as_str().unwrap();
            let v = if n == "egg" {
                json!(["dairy_and_egg", "egg"])
            } else {
                Value::Null
            };
            results.insert(n.to_string(), v);
        }
        ResponseTemplate::new(200).set_body_json(json!({ "results": results }))
    }
}

async fn mount_categories(server: &MockServer, expect: u64) {
    Mock::given(method("GET"))
        .and(path("/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": [
                {"slug": "dairy_and_egg", "name": "Dairy and egg", "parent": null},
                {"slug": "egg", "name": "Egg", "parent": "dairy_and_egg"},
                {"slug": "fish", "name": "Fish", "parent": null}
            ]
        })))
        .expect(expect)
        .mount(server)
        .await;
}

async fn mount_classify(server: &MockServer, expect: u64) {
    Mock::given(method("POST"))
        .and(path("/categories/classify"))
        .respond_with(ClassifyResponder)
        .expect(expect)
        .mount(server)
        .await;
}

async fn mount_check_never(server: &MockServer) {
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(r"^/categories/[^/]+/check$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"in_category": true})))
        .expect(0)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn classify_dedupes_and_chunks_by_200() {
    let server = MockServer::start().await;
    mount_classify(&server, 2).await;
    let url = server.uri();
    let out = tokio::task::spawn_blocking(move || {
        let mut input: Vec<String> = (0..250).map(|i| format!("ing{i}")).collect();
        input.extend(names(&["egg", "egg", "ing0", "ing1"])); // duplicates
        Client::new(url).classify(&input).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(out.len(), 251); // 250 + egg
    assert_eq!(
        out["egg"],
        Some(vec!["dairy_and_egg".to_string(), "egg".to_string()])
    );
    assert_eq!(out["ing0"], None);
    let reqs = server.received_requests().await.unwrap();
    let sizes: Vec<usize> = reqs
        .iter()
        .map(|r| {
            let b: Value = serde_json::from_slice(&r.body).unwrap();
            b["ingredients"].as_array().unwrap().len()
        })
        .collect();
    assert_eq!(sizes, vec![200, 51]);
}

#[tokio::test(flavor = "multi_thread")]
async fn classify_empty_makes_no_request() {
    let server = MockServer::start().await;
    mount_classify(&server, 0).await;
    let url = server.uri();
    let out = tokio::task::spawn_blocking(move || Client::new(url).classify(&[]).unwrap())
        .await
        .unwrap();
    assert!(out.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn prefetch_then_checks_make_no_further_requests() {
    let server = MockServer::start().await;
    mount_categories(&server, 1).await;
    mount_classify(&server, 1).await;
    mount_check_never(&server).await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url).cached();
        c.prefetch_categories(&names(&["egg", "zzz"])).unwrap();
        for _ in 0..5 {
            assert!(c.is_in_category("egg", "egg").unwrap());
            assert!(c.is_in_category("egg", "dairy_and_egg").unwrap());
            assert!(!c.is_in_category("egg", "fish").unwrap());
            assert!(!c.is_in_category("zzz", "fish").unwrap());
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn prefetch_skips_cached_names() {
    let server = MockServer::start().await;
    mount_categories(&server, 1).await;
    mount_classify(&server, 2).await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url).cached();
        c.prefetch_categories(&names(&["egg"])).unwrap();
        c.prefetch_categories(&names(&["egg"])).unwrap(); // nothing new: no POST
        c.prefetch_categories(&names(&["egg", "salmon"])).unwrap(); // only salmon
    })
    .await
    .unwrap();
    let reqs = server.received_requests().await.unwrap();
    let bodies: Vec<Value> = reqs
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[1]["ingredients"], json!(["salmon"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn prefetch_is_noop_without_cache() {
    let server = MockServer::start().await;
    mount_categories(&server, 0).await;
    mount_classify(&server, 0).await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        Client::new(url)
            .prefetch_categories(&names(&["egg"]))
            .unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn lazy_path_one_classify_per_new_ingredient() {
    let server = MockServer::start().await;
    mount_categories(&server, 1).await;
    mount_classify(&server, 2).await; // egg, zzz
    mount_check_never(&server).await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url).cached();
        for slug in ["egg", "dairy_and_egg", "fish", "egg", "fish"] {
            c.is_in_category("egg", slug).unwrap();
        }
        for slug in ["egg", "fish", "dairy_and_egg"] {
            assert!(!c.is_in_category("zzz", slug).unwrap());
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_slug_is_category_not_found() {
    let server = MockServer::start().await;
    mount_categories(&server, 1).await;
    mount_classify(&server, 0).await;
    let url = server.uri();
    let err = tokio::task::spawn_blocking(move || {
        Client::new(url)
            .cached()
            .is_in_category("egg", "nope")
            .unwrap_err()
    })
    .await
    .unwrap();
    match err {
        ClientError::CategoryNotFound(d) => assert_eq!(d, "category not found: nope"),
        other => panic!("expected CategoryNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn falls_back_to_check_when_classify_404() {
    let server = MockServer::start().await;
    mount_categories(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/categories/classify"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1) // flag set after first 404: never retried
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/categories/egg/check"))
        .and(query_param("ingredient", "egg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"in_category": true})))
        .expect(1) // per-(ingredient,slug) cache still applies
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/categories/fish/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"in_category": false})))
        .expect(1)
        .mount(&server)
        .await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url).cached();
        assert!(c.is_in_category("egg", "egg").unwrap());
        assert!(c.is_in_category("egg", "egg").unwrap());
        assert!(!c.is_in_category("egg", "fish").unwrap());
        c.prefetch_categories(&names(&["x"])).unwrap(); // no-op now
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn falls_back_to_check_when_categories_405() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/categories"))
        .respond_with(ResponseTemplate::new(405))
        .expect(1)
        .mount(&server)
        .await;
    mount_classify(&server, 0).await;
    Mock::given(method("GET"))
        .and(path("/categories/egg/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"in_category": true})))
        .expect(2)
        .mount(&server)
        .await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url).cached();
        c.prefetch_categories(&names(&["egg"])).unwrap();
        assert!(c.is_in_category("egg", "egg").unwrap());
        assert!(c.is_in_category("egg2", "egg").unwrap());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn uncached_client_uses_check_exactly_as_before() {
    let server = MockServer::start().await;
    mount_categories(&server, 0).await;
    mount_classify(&server, 0).await;
    Mock::given(method("GET"))
        .and(path("/categories/egg/check"))
        .and(query_param("ingredient", "egg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"in_category": true})))
        .expect(2) // no cache: every call hits the server
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/categories/nope/check"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "code": "category_not_found", "detail": "category not found: nope"
        })))
        .mount(&server)
        .await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(url);
        assert!(c.is_in_category("egg", "egg").unwrap());
        assert!(c.is_in_category("egg", "egg").unwrap());
        match c.is_in_category("egg", "nope").unwrap_err() {
            ClientError::CategoryNotFound(d) => assert_eq!(d, "category not found: nope"),
            other => panic!("{other:?}"),
        }
    })
    .await
    .unwrap();
}
