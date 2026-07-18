//! Closed-source HTTP client for the cooklang nutrition service.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NutritionFacts {
    pub kcal: f64,
    pub protein_g: f64,
    pub fat_g: f64,
    pub carb_g: f64,
    pub fiber_g: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Warning {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllergenEntry {
    pub class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllergenBlock {
    /// `verified` or `unverified`. Unverified means nothing known — NOT free-of.
    pub status: String,
    pub contains: Vec<AllergenEntry>,
    pub view: String,
}

/// Batch rollup: `contains` unions VERIFIED items only — a floor, not a
/// ceiling, whenever `unverified_ingredients` is non-empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllergenSummary {
    pub contains: Vec<AllergenEntry>,
    pub unverified_ingredients: Vec<String>,
    pub view: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateItem {
    pub ingredient: String,
    pub amount: f64,
    pub unit: String,
    pub prep: String,
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResponse {
    pub items: Vec<NutritionResponse>,
    pub failures: Vec<AggregateFailure>,
    pub totals: Totals,
    pub confidence_breakdown: ConfidenceBreakdown,
    #[serde(default)]
    pub matched_exclusions: Vec<String>,
    #[serde(default)]
    pub unresolved_exclusions: Vec<String>,
    #[serde(default)]
    pub reference_standard: Option<String>,
    #[serde(default)]
    pub reference_intakes: std::collections::BTreeMap<String, f64>,
    /// Absent when talking to a pre-allergens service.
    #[serde(default)]
    pub allergen_summary: Option<AllergenSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NutritionResponse {
    pub ingredient: String,
    pub preparation: String,
    pub amount: AmountBlock,
    pub macros: MacroTotals,
    pub micros: std::collections::BTreeMap<String, f64>,
    pub vitamins: std::collections::BTreeMap<String, f64>,
    pub source: String,
    pub confidence: String,
    pub warnings: Vec<Warning>,
    #[serde(default)]
    pub matched_exclusions: Vec<String>,
    /// Absent when talking to a pre-allergens service.
    #[serde(default)]
    pub allergens: Option<AllergenBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmountBlock {
    pub value: f64,
    pub unit: String,
    pub mass_g: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Totals {
    pub mass_g: f64,
    pub macros: MacroTotals,
    pub micros: std::collections::BTreeMap<String, f64>,
    pub vitamins: std::collections::BTreeMap<String, f64>,
    pub confidence: String,
    /// Mass-weighted confidence (spec §12.6). `#[serde(default)]` keeps older
    /// service responses deserialising.
    #[serde(default)]
    pub confidence_weighted: String,
    pub is_partial: bool,
    pub included_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroTotals {
    pub kcal: f64,
    pub protein_g: f64,
    pub fat_g: f64,
    pub carb_g: f64,
    pub fiber_g: f64,
    pub sugar_g: f64,
    pub sat_fat_g: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateFailure {
    pub index: usize,
    pub ingredient: String,
    pub error: AggregateFailureError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateFailureError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceBreakdown {
    pub confirmed_items: usize,
    pub partial_items: usize,
    pub estimated_items: usize,
    pub estimated_ingredients: Vec<String>,
    // Spec §7.2: powers the "…affect <N% of total micronutrients" report line.
    pub estimated_share_of_micronutrients: Option<f64>,
}

/// Response from `GET /ingredients/lookup`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupResponse {
    pub ingredient: LookupIngredient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupIngredient {
    pub id: i64,
    pub canonical_name: String,
    pub categories: Vec<String>,
    pub available_preparations: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("ingredient not found: {0}")]
    NotFound(String),
    #[error("preparation not available: {0}")]
    PrepNotAvailable(String),
    #[error("unsupported unit: {0}")]
    UnsupportedUnit(String),
    #[error("invalid amount: {0}")]
    BadAmount(String),
    #[error("category not found: {0}")]
    CategoryNotFound(String),
    #[error("authentication required: {0}")]
    Unauthorized(String),
    #[error("{0}")]
    SubscriptionRequired(String),
    #[error("nutrition service unavailable: {0}")]
    Unavailable(String),
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("server error: status {0}")]
    Server(u16),
}

/// Optional in-memory caches keyed by the request parameters (spec §12.3).
/// Off by default; opt in with [`Client::cached`]. Cuts repeat API calls for
/// ingredients that recur across a recipe or plan (e.g. `category_servings`
/// checking the same ingredient in many meals).
#[derive(Default)]
struct ClientCache {
    nutrition: std::sync::Mutex<std::collections::HashMap<String, NutritionFacts>>,
    category: std::sync::Mutex<std::collections::HashMap<String, bool>>,
}

/// A single standard's daily reference-intake table (`nutrient_slug` → amount).
type ReferenceTable = std::collections::BTreeMap<String, f64>;
/// Memo of reference tables keyed by standard. Always-on (unlike [`ClientCache`])
/// because the data is static per standard, so there's no opt-in to gate.
type ReferenceCache =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, ReferenceTable>>>;

pub struct Client {
    base_url: String,
    http: reqwest::blocking::Client,
    cache: Option<std::sync::Arc<ClientCache>>,
    auth_token: Option<String>,
    /// Always-on memo of static per-standard reference-intake tables (no opt-in,
    /// unlike `cache`): fetched once per standard and reused for the client's life.
    reference_cache: ReferenceCache,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            cache: None,
            auth_token: None,
            reference_cache: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Enable the in-memory result cache (spec §12.3). Safe across threads;
    /// cache lives for the lifetime of this `Client`.
    pub fn cached(mut self) -> Self {
        self.cache = Some(std::sync::Arc::new(ClientCache::default()));
        self
    }

    /// Forward `token` as `Authorization: Bearer <token>` on every request
    /// (the caller's Cook Pro token). No header is sent when unset.
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Apply the auth header if a token is configured.
    fn auth(&self, rb: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.auth_token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    pub fn nutrition_for(
        &self,
        name: &str,
        amount: f64,
        unit: &str,
        prep: &str,
    ) -> Result<NutritionFacts, ClientError> {
        let cache_key = format!("{name}|{amount}|{unit}|{prep}");
        if let Some(cache) = &self.cache {
            if let Some(hit) = cache.nutrition.lock().unwrap().get(&cache_key).cloned() {
                return Ok(hit);
            }
        }
        let url = format!("{}/nutrition", self.base_url.trim_end_matches('/'));
        let resp = self
            .auth(self.http.get(&url).query(&[
                ("ingredient", name),
                ("amount", &amount.to_string()),
                ("unit", unit),
                ("prep", prep),
            ]))
            .send()?;

        let status = resp.status();
        if status.is_success() {
            #[derive(Deserialize)]
            struct Envelope {
                macros: NutritionFacts,
            }
            let env: Envelope = resp.json()?;
            if let Some(cache) = &self.cache {
                cache
                    .nutrition
                    .lock()
                    .unwrap()
                    .insert(cache_key, env.macros.clone());
            }
            return Ok(env.macros);
        }

        Err(parse_error(resp, status.as_u16()))
    }

    pub fn aggregate(
        &self,
        items: &[AggregateItem],
        exclusions: &[String],
        standard: Option<&str>,
    ) -> Result<AggregateResponse, ClientError> {
        let url = format!("{}/aggregate", self.base_url.trim_end_matches('/'));
        let mut body = serde_json::json!({ "items": items, "exclusions": exclusions });
        if let Some(s) = standard {
            body["reference"] = serde_json::Value::String(s.to_string());
        }
        let resp = self.auth(self.http.post(&url).json(&body)).send()?;

        let status = resp.status();
        if status.is_success() {
            let agg: AggregateResponse = resp.json()?;
            return Ok(agg);
        }

        Err(parse_error(resp, status.as_u16()))
    }

    /// Daily reference intakes for `standard` (default `fda`). Fetched once per
    /// standard and memoised for the client's lifetime.
    pub fn reference_intakes(&self, standard: &str) -> Result<ReferenceTable, ClientError> {
        if let Some(hit) = self.reference_cache.lock().unwrap().get(standard).cloned() {
            return Ok(hit);
        }
        let url = format!("{}/reference-intakes", self.base_url.trim_end_matches('/'));
        let resp = self
            .auth(self.http.get(&url).query(&[("standard", standard)]))
            .send()?;
        let status = resp.status();
        if status.is_success() {
            let mut tables: std::collections::BTreeMap<String, ReferenceTable> = resp.json()?;
            // A 200 body that omits the requested standard is a server-contract
            // violation, not a valid "no DVs" state — surface it rather than
            // silently caching an empty table that reads as "no DV everywhere".
            let table = tables
                .remove(standard)
                .ok_or(ClientError::Server(status.as_u16()))?;
            self.reference_cache
                .lock()
                .unwrap()
                .insert(standard.to_string(), table.clone());
            return Ok(table);
        }
        Err(parse_error(resp, status.as_u16()))
    }

    /// `GET /ingredients/lookup` — resolve a name/alias/translation to a
    /// canonical ingredient with its categories and available preparations.
    pub fn lookup(&self, q: &str, lang: Option<&str>) -> Result<LookupResponse, ClientError> {
        let url = format!("{}/ingredients/lookup", self.base_url.trim_end_matches('/'));
        let mut query: Vec<(&str, &str)> = vec![("q", q)];
        if let Some(lang) = lang {
            query.push(("lang", lang));
        }
        let resp = self.auth(self.http.get(&url).query(&query)).send()?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp.json()?);
        }
        Err(parse_error(resp, status.as_u16()))
    }

    /// `GET /categories/{slug}/check` — whether an ingredient is in a category
    /// (tree-aware). An unknown ingredient resolves to `false`; an unknown
    /// category slug surfaces as `CategoryNotFound` so template typos are caught.
    pub fn is_in_category(&self, ingredient: &str, slug: &str) -> Result<bool, ClientError> {
        let cache_key = format!("{ingredient}|{slug}");
        if let Some(cache) = &self.cache {
            if let Some(hit) = cache.category.lock().unwrap().get(&cache_key).copied() {
                return Ok(hit);
            }
        }
        let url = format!(
            "{}/categories/{}/check",
            self.base_url.trim_end_matches('/'),
            slug
        );
        let resp = self
            .auth(self.http.get(&url).query(&[("ingredient", ingredient)]))
            .send()?;
        let status = resp.status();
        if status.is_success() {
            #[derive(Deserialize)]
            struct Envelope {
                in_category: bool,
            }
            let env: Envelope = resp.json()?;
            if let Some(cache) = &self.cache {
                cache
                    .category
                    .lock()
                    .unwrap()
                    .insert(cache_key, env.in_category);
            }
            return Ok(env.in_category);
        }
        Err(match parse_error(resp, status.as_u16()) {
            // Unknown ingredient → simply not a member.
            ClientError::NotFound(_) => return Ok(false),
            other => other,
        })
    }

    /// `GET /convert` — convert `amount` from one unit to another, returning
    /// the numeric result. `ingredient` is required for volume/count units.
    pub fn convert(
        &self,
        amount: f64,
        from: &str,
        to: &str,
        ingredient: Option<&str>,
        prep: Option<&str>,
        region: Option<&str>,
    ) -> Result<f64, ClientError> {
        let url = format!("{}/convert", self.base_url.trim_end_matches('/'));
        let amount_s = amount.to_string();
        let mut query: Vec<(&str, &str)> = vec![("amount", &amount_s), ("from", from), ("to", to)];
        if let Some(i) = ingredient {
            query.push(("ingredient", i));
        }
        if let Some(p) = prep {
            query.push(("prep", p));
        }
        if let Some(r) = region {
            query.push(("region", r));
        }
        let resp = self.auth(self.http.get(&url).query(&query)).send()?;
        let status = resp.status();
        if status.is_success() {
            #[derive(Deserialize)]
            struct Envelope {
                result: f64,
            }
            let env: Envelope = resp.json()?;
            return Ok(env.result);
        }
        Err(parse_error(resp, status.as_u16()))
    }
}

/// Parse a service RFC 9457 Problem Details body into a `ClientError`,
/// falling back to a `Server` error when the body isn't the expected shape.
fn parse_error(resp: reqwest::blocking::Response, raw_status: u16) -> ClientError {
    #[derive(Deserialize)]
    struct Problem {
        code: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        suggestions: Vec<String>,
    }
    if let Ok(p) = resp.json::<Problem>() {
        // For auth/subscription errors, append the first actionable suggestion
        // (e.g. the upgrade link) so it surfaces in the rendered report message.
        let msg = match p.suggestions.first() {
            Some(s) if !p.detail.is_empty() => format!("{} ({})", p.detail, s),
            Some(s) => s.clone(),
            None => p.detail.clone(),
        };
        return match p.code.as_str() {
            "ingredient_not_found" => ClientError::NotFound(p.detail),
            "preparation_not_available" => ClientError::PrepNotAvailable(p.detail),
            "unknown_unit" => ClientError::UnsupportedUnit(p.detail),
            "invalid_amount" => ClientError::BadAmount(p.detail),
            "density_unavailable" => ClientError::UnsupportedUnit(p.detail),
            "category_not_found" => ClientError::CategoryNotFound(p.detail),
            "unauthorized" => ClientError::Unauthorized(msg),
            "subscription_required" => ClientError::SubscriptionRequired(msg),
            "upstream_unavailable" => ClientError::Unavailable(msg),
            _ => ClientError::Server(raw_status),
        };
    }
    ClientError::Server(raw_status)
}

#[cfg(test)]
mod deser_tests {
    use super::*;

    // The reference block is top-level on the aggregate response (the service
    // never repeats it per item), so it's `AggregateResponse` — not the per-item
    // `NutritionResponse` — that mirrors `reference_standard`/`reference_intakes`.
    #[test]
    fn aggregate_response_deserializes_reference_block() {
        let json = r#"{
            "items":[{
                "ingredient":"salmon","preparation":"raw",
                "amount":{"value":100.0,"unit":"g","mass_g":100.0},
                "macros":{"kcal":1.0,"protein_g":1.0,"fat_g":1.0,"carb_g":1.0,"fiber_g":1.0,"sugar_g":1.0,"sat_fat_g":1.0},
                "micros":{},"vitamins":{},"source":"usda","confidence":"confirmed","warnings":[]
            }],
            "failures":[],
            "totals":{"mass_g":100.0,"macros":{"kcal":1.0,"protein_g":1.0,"fat_g":1.0,"carb_g":1.0,"fiber_g":1.0,"sugar_g":1.0,"sat_fat_g":1.0},"micros":{},"vitamins":{},"confidence":"confirmed","confidence_weighted":"confirmed","is_partial":false,"included_count":1,"failed_count":0},
            "confidence_breakdown":{"confirmed_items":1,"partial_items":0,"estimated_items":0,"estimated_ingredients":[],"estimated_share_of_micronutrients":null},
            "reference_standard":"fda","reference_intakes":{"sodium_mg":2300.0}
        }"#;
        let parsed: AggregateResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.reference_standard.as_deref(), Some("fda"));
        assert_eq!(parsed.reference_intakes.get("sodium_mg"), Some(&2300.0));
    }

    #[test]
    fn nutrition_response_deserializes_allergens_block() {
        let json = r#"{
            "ingredient":"flour","preparation":"raw",
            "amount":{"value":100.0,"unit":"g","mass_g":100.0},
            "macros":{"kcal":1.0,"protein_g":1.0,"fat_g":1.0,"carb_g":1.0,"fiber_g":1.0,"sugar_g":1.0,"sat_fat_g":1.0},
            "micros":{},"vitamins":{},"source":"usda","confidence":"confirmed","warnings":[],
            "allergens":{"status":"verified","contains":[{"class":"gluten","subtype":"wheat","label":"Wheat"}],"view":"fda"}
        }"#;
        let parsed: NutritionResponse = serde_json::from_str(json).unwrap();
        let block = parsed.allergens.unwrap();
        assert_eq!(block.status, "verified");
        assert_eq!(block.contains[0].label, "Wheat");
        assert_eq!(block.contains[0].subtype.as_deref(), Some("wheat"));
        assert_eq!(block.view, "fda");
    }

    #[test]
    fn unverified_allergens_block_deserializes_with_empty_contains() {
        let json = r#"{
            "ingredient":"saffron","preparation":"raw",
            "amount":{"value":1.0,"unit":"g","mass_g":1.0},
            "macros":{"kcal":1.0,"protein_g":1.0,"fat_g":1.0,"carb_g":1.0,"fiber_g":1.0,"sugar_g":1.0,"sat_fat_g":1.0},
            "micros":{},"vitamins":{},"source":"usda","confidence":"confirmed","warnings":[],
            "allergens":{"status":"unverified","contains":[],"view":"fda"}
        }"#;
        let parsed: NutritionResponse = serde_json::from_str(json).unwrap();
        let block = parsed.allergens.unwrap();
        assert_eq!(block.status, "unverified");
        assert!(block.contains.is_empty());
        assert_eq!(block.view, "fda");
    }

    #[test]
    fn old_service_without_allergens_still_deserializes() {
        let json = r#"{
            "ingredient":"flour","preparation":"raw",
            "amount":{"value":100.0,"unit":"g","mass_g":100.0},
            "macros":{"kcal":1.0,"protein_g":1.0,"fat_g":1.0,"carb_g":1.0,"fiber_g":1.0,"sugar_g":1.0,"sat_fat_g":1.0},
            "micros":{},"vitamins":{},"source":"usda","confidence":"confirmed","warnings":[]
        }"#;
        let parsed: NutritionResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.allergens.is_none());
    }

    #[test]
    fn aggregate_response_deserializes_allergen_summary() {
        // Reuse the JSON from aggregate_response_deserializes_reference_block
        // with this appended before the closing brace:
        //   ,"allergen_summary":{"contains":[{"class":"milk","label":"Milk"}],
        //    "unverified_ingredients":["saffron"],"view":"fda"}
        let json = r#"{
            "items":[],"failures":[],
            "totals":{"mass_g":0.0,"macros":{"kcal":0.0,"protein_g":0.0,"fat_g":0.0,"carb_g":0.0,"fiber_g":0.0,"sugar_g":0.0,"sat_fat_g":0.0},"micros":{},"vitamins":{},"confidence":"confirmed","confidence_weighted":"confirmed","is_partial":false,"included_count":0,"failed_count":0},
            "confidence_breakdown":{"confirmed_items":0,"partial_items":0,"estimated_items":0,"estimated_ingredients":[],"estimated_share_of_micronutrients":null},
            "allergen_summary":{"contains":[{"class":"milk","label":"Milk"}],"unverified_ingredients":["saffron"],"view":"fda"}
        }"#;
        let parsed: AggregateResponse = serde_json::from_str(json).unwrap();
        let s = parsed.allergen_summary.unwrap();
        assert_eq!(s.contains[0].class, "milk");
        assert_eq!(s.contains[0].subtype, None);
        assert_eq!(s.unverified_ingredients, vec!["saffron"]);
    }
}

#[cfg(test)]
mod auth_header_tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ok_body() -> serde_json::Value {
        serde_json::json!({
            "macros": { "kcal": 1.0, "protein_g": 0.0, "fat_g": 0.0, "carb_g": 0.0, "fiber_g": 0.0 }
        })
    }

    #[tokio::test]
    async fn sends_bearer_when_token_set() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nutrition"))
            .and(header("authorization", "Bearer tok-xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .expect(1)
            .mount(&server)
            .await;
        let base = server.uri();

        tokio::task::spawn_blocking(move || {
            let client = Client::new(base).with_auth_token("tok-xyz");
            client
                .nutrition_for("salmon", 100.0, "g", "cooked")
                .unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn omits_auth_when_no_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nutrition"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .expect(1)
            .mount(&server)
            .await;
        let base = server.uri();

        tokio::task::spawn_blocking(move || {
            let client = Client::new(base);
            client
                .nutrition_for("salmon", 100.0, "g", "cooked")
                .unwrap();
        })
        .await
        .unwrap();

        // Inspect the recorded request directly: with no token configured, no
        // Authorization header must be sent. (wiremock 0.6's matchers can't
        // assert header-absence, so we verify the actual request instead.)
        let requests = server.received_requests().await.expect("requests recorded");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].headers.get("authorization").is_none(),
            "no Authorization header should be sent when no token is configured"
        );
    }
}

#[cfg(test)]
mod auth_error_mapping_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn problem_server(status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nutrition"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn maps_subscription_required_with_suggestion() {
        let server = problem_server(
            403,
            serde_json::json!({
                "code": "subscription_required",
                "detail": "active Cook Pro subscription required",
                "suggestions": ["subscribe to Cook Pro at https://cook.md/pro"]
            }),
        )
        .await;
        let base = server.uri();
        let err = tokio::task::spawn_blocking(move || {
            Client::new(base)
                .nutrition_for("salmon", 100.0, "g", "cooked")
                .unwrap_err()
        })
        .await
        .unwrap();
        match err {
            ClientError::SubscriptionRequired(msg) => {
                assert!(msg.contains("active Cook Pro subscription required"));
                assert!(msg.contains("https://cook.md/pro"));
            }
            other => panic!("expected SubscriptionRequired, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maps_unauthorized() {
        let server = problem_server(
            401,
            serde_json::json!({
                "code": "unauthorized", "detail": "API key or Cook Pro token required"
            }),
        )
        .await;
        let base = server.uri();
        let err = tokio::task::spawn_blocking(move || {
            Client::new(base)
                .nutrition_for("salmon", 100.0, "g", "cooked")
                .unwrap_err()
        })
        .await
        .unwrap();
        match err {
            ClientError::Unauthorized(msg) => {
                assert!(msg.contains("API key or Cook Pro token required"))
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maps_upstream_unavailable() {
        let server = problem_server(503, serde_json::json!({
            "code": "upstream_unavailable", "detail": "subscription service unavailable, retry shortly"
        })).await;
        let base = server.uri();
        let err = tokio::task::spawn_blocking(move || {
            Client::new(base)
                .nutrition_for("salmon", 100.0, "g", "cooked")
                .unwrap_err()
        })
        .await
        .unwrap();
        match err {
            ClientError::Unavailable(msg) => {
                assert!(msg.contains("subscription service unavailable"))
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}
