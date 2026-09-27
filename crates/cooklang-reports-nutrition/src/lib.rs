//! Minijinja extension for cooklang-reports that registers nutrition
//! functions (`nutrition_for`, `compare`, `total_calories`, ...).

pub mod checks;
pub mod client;
pub mod failures;
pub mod plan;
pub mod report;

use std::sync::Arc;

use cooklang_reports::ConfigExtension;
use cookmd_nutrition_client::{
    AggregateItem, AggregateResponse, Client, ConfidenceBreakdown, MacroTotals, NutritionFacts,
    Totals,
};
use minijinja::Environment;

/// Pure helper: compare(actual, target, op). Slice supports gte/lte/eq.
/// Any other op returns false (the wider macro library lands later).
pub fn compare_fn(actual: f64, target: f64, op: String) -> bool {
    match op.as_str() {
        "gte" => actual >= target,
        "lte" => actual <= target,
        "eq" => (actual - target).abs() < f64::EPSILON,
        _ => false,
    }
}

/// Within-tolerance check: `|actual - base| <= base * tol_pct / 100`.
/// `tol_pct` is a percent (e.g. `10.0` for ±10%). When `base == 0.0`,
/// requires exact-zero `actual` (avoids divide-by-zero). Negative
/// `tol_pct` or non-finite inputs return `false`.
pub fn within_tol(actual: f64, base: f64, tol_pct: f64) -> bool {
    if !actual.is_finite() || !base.is_finite() || !tol_pct.is_finite() {
        return false;
    }
    if tol_pct < 0.0 {
        return false;
    }
    if base == 0.0 {
        return actual == 0.0;
    }
    let allowed = base.abs() * tol_pct / 100.0;
    (actual - base).abs() <= allowed
}

/// Value of a unicode vulgar-fraction character (`½` -> 0.5), if it is one.
fn vulgar_fraction(c: char) -> Option<f64> {
    let (n, d) = match c {
        '½' => (1.0, 2.0),
        '⅓' => (1.0, 3.0),
        '⅔' => (2.0, 3.0),
        '¼' => (1.0, 4.0),
        '¾' => (3.0, 4.0),
        '⅕' => (1.0, 5.0),
        '⅖' => (2.0, 5.0),
        '⅗' => (3.0, 5.0),
        '⅘' => (4.0, 5.0),
        '⅙' => (1.0, 6.0),
        '⅚' => (5.0, 6.0),
        '⅐' => (1.0, 7.0),
        '⅛' => (1.0, 8.0),
        '⅜' => (3.0, 8.0),
        '⅝' => (5.0, 8.0),
        '⅞' => (7.0, 8.0),
        '⅑' => (1.0, 9.0),
        '⅒' => (1.0, 10.0),
        _ => return None,
    };
    Some(n / d)
}

/// Split off a leading run of ASCII digits: `("12", "rest")`.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

/// Parse a leading fraction: a unicode vulgar fraction (`½`) or `a/b` with
/// integer `a`, `b` and `b != 0` (`⁄`, the fraction slash, also accepted).
/// Returns the value and the unparsed remainder.
fn leading_fraction(s: &str) -> Option<(f64, &str)> {
    let mut chars = s.chars();
    if let Some(v) = chars.next().and_then(vulgar_fraction) {
        return Some((v, chars.as_str()));
    }
    let (num, rest) = split_digits(s);
    if num.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(['/', '⁄'])?;
    let (den, rest) = split_digits(rest);
    let den: f64 = den.parse().ok()?;
    if den == 0.0 {
        return None;
    }
    Some((num.parse::<f64>().ok()? / den, rest))
}

/// Parse the leading number of a text quantity the Cooklang parser left
/// unparsed, returning its value when finite and positive.
///
/// The number may be an integer or decimal (`2`, `2.25`), a fraction (`3/4`,
/// `½`) or a mixed number (`1 1/2`, `1½`, `1 ½`, `1 and 1/2`). Anything after
/// it is ignored, so a range (`1-2`, `3/4-1`, `½-¾`, `1 to 2`, `1–2`) yields
/// its lower bound and `2 large` yields 2.
fn parse_amount_text(s: &str) -> Option<f64> {
    let t = s.trim();
    let value = if let Some((frac, _)) = leading_fraction(t) {
        frac
    } else {
        let (int, rest) = split_digits(t);
        let (whole, rest, is_integer) = match rest.strip_prefix('.') {
            Some(after_dot) => {
                let (decimals, rest) = split_digits(after_dot);
                let len = int.len() + 1 + decimals.len();
                (t[..len].parse::<f64>().ok()?, rest, false)
            }
            None => (int.parse::<f64>().ok()?, rest, true),
        };
        // Mixed number: an integer followed by a fraction, optionally joined
        // by whitespace and/or `and`.
        let tail = rest.trim_start();
        let tail = match tail.strip_prefix("and") {
            Some(after_and) if after_and.starts_with(char::is_whitespace) => after_and.trim_start(),
            _ => tail,
        };
        match leading_fraction(tail) {
            Some((frac, _)) if is_integer => whole + frac,
            _ => whole,
        }
    };
    (value.is_finite() && value > 0.0).then_some(value)
}

/// Interpret a cooklang quantity value string + declared unit into a numeric
/// (amount, unit). Recipes use textual amounts heavily (`to taste`, `pinch`,
/// ranges like `1-2`); rather than DROPPING those ingredients we map them to a
/// nominal amount so the ingredient still resolves — a seasoning "to taste"
/// then contributes a negligible (pinch-sized) mass instead of vanishing.
fn interpret_amount(value_str: &str, unit: &str) -> (f64, String) {
    let v = value_str.trim();
    if let Ok(n) = v.parse::<f64>() {
        if n.is_finite() && n > 0.0 {
            return (n, unit.to_string());
        }
    }
    if let Some(n) = parse_amount_text(v) {
        return (n, unit.to_string());
    }
    match v.to_lowercase().as_str() {
        "pinch" | "a pinch" | "pinches" => (1.0, "pinch".to_string()),
        "dash" | "a dash" => (1.0, "dash".to_string()),
        "to taste" | "as needed" | "as required" | "to serve" | "for serving" | "for garnish"
        | "to garnish" | "for dusting" | "optional" => (1.0, "pinch".to_string()),
        // Unrecognised text with a real unit -> keep the unit (amount 1).
        // Unrecognised text with NO unit (incl. empty `{}`) -> nominal pinch, so
        // the item resolves via the global pinch->g edge for any food. (Numeric
        // bare counts like `{1}` never reach here; they stay bare-count -> default
        // portion, so countable produce is unaffected.)
        _ if unit.is_empty() => (1.0, "pinch".to_string()),
        _ => (1.0, unit.to_string()),
    }
}

fn ingredients_to_items(
    ingredients: &minijinja::Value,
) -> Result<Vec<AggregateItem>, minijinja::Error> {
    let mut items = Vec::new();
    for ing in ingredients.try_iter()? {
        // Skip recipe references (sub-recipe placeholders, not real food)
        if let Ok(reference) = ing.get_attr("reference") {
            if reference.is_true() {
                continue;
            }
        }

        // Strip a leading cooklang optional-marker (`?`) that some shapes leak
        // into the name, plus surrounding whitespace.
        let name = ing
            .get_attr("name")?
            .to_string()
            .trim_start_matches('?')
            .trim()
            .to_string();

        // Two supported shapes:
        // 1. cooklang-reports Ingredient:    .quantity.value / .quantity.unit / .note
        // 2. cooklang-reports IngredientListItem: .quantities[0].value / .quantities[0].unit
        //
        // get_attr returns Ok(UNDEFINED) when the key exists on the object but has no value,
        // so we must filter out undefined before choosing the branch. Missing /
        // non-numeric amounts default to a bare count rather than dropping the item.
        let quantity_val = ing
            .get_attr("quantity")
            .ok()
            .filter(|q| !q.is_undefined() && !q.is_none());
        let (amount, unit, prep) = if let Some(quantity) = quantity_val {
            let value_str = quantity
                .get_attr("value")
                .ok()
                .filter(|v| !v.is_undefined() && !v.is_none())
                .map(|v| v.to_string())
                .unwrap_or_default();
            let unit = quantity
                .get_attr("unit")
                .ok()
                .filter(|v| !v.is_undefined() && !v.is_none())
                .map(|u| u.to_string())
                .unwrap_or_default();
            let (amount, unit) = interpret_amount(&value_str, &unit);
            let prep = ing
                .get_attr("note")
                .ok()
                .map(|n| n.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            (amount, unit, prep)
        } else if let Ok(quantities) = ing.get_attr("quantities") {
            let first = quantities
                .get_item(&minijinja::Value::from(0usize))
                .ok()
                .filter(|q| !q.is_undefined() && !q.is_none());
            match first {
                Some(q) => {
                    let value_str = q
                        .get_attr("value")
                        .ok()
                        .filter(|v| !v.is_undefined() && !v.is_none())
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let unit = q
                        .get_attr("unit")
                        .ok()
                        .filter(|v| !v.is_none() && !v.is_undefined())
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    let (amount, unit) = interpret_amount(&value_str, &unit);
                    (amount, unit, String::new())
                }
                // No quantity at all (`@salt{}`) -> nominal pinch so any food
                // resolves via the global pinch->g edge.
                None => (1.0, "pinch".to_string(), String::new()),
            }
        } else {
            // Neither shape carried a quantity -> nominal pinch.
            (1.0, "pinch".to_string(), String::new())
        };

        // `whole`/`whole piece` are count qualifiers, not real units
        // (`@chicken{1%whole}`); drop to a bare count -> default portion.
        let unit = match unit.trim().to_lowercase().as_str() {
            "whole" | "whole piece" | "each" | "count" => String::new(),
            _ => unit,
        };

        items.push(AggregateItem {
            ingredient: name,
            amount,
            unit,
            prep,
            region: "us".into(),
        });
    }
    Ok(items)
}

/// Record every per-ingredient failure from an aggregate response into the
/// tracker. Called once per response received — each jinja function issues its
/// own aggregate call, so there is no shared fetch path to double-record.
fn record_failures(failures: &crate::failures::FailureTracker, resp: &AggregateResponse) {
    for f in &resp.failures {
        failures.record(serde_json::to_value(f).unwrap_or_default());
    }
}

fn empty_aggregate_response() -> AggregateResponse {
    AggregateResponse {
        items: Vec::new(),
        failures: Vec::new(),
        totals: Totals {
            mass_g: 0.0,
            macros: MacroTotals {
                kcal: 0.0,
                protein_g: 0.0,
                fat_g: 0.0,
                carb_g: 0.0,
                fiber_g: 0.0,
                sugar_g: 0.0,
                sat_fat_g: 0.0,
            },
            micros: std::collections::BTreeMap::new(),
            vitamins: std::collections::BTreeMap::new(),
            confidence: "confirmed".to_string(),
            confidence_weighted: "confirmed".to_string(),
            is_partial: false,
            included_count: 0,
            failed_count: 0,
        },
        confidence_breakdown: ConfidenceBreakdown {
            confirmed_items: 0,
            partial_items: 0,
            estimated_items: 0,
            estimated_ingredients: Vec::new(),
            estimated_share_of_micronutrients: None,
        },
        matched_exclusions: Vec::new(),
        unresolved_exclusions: Vec::new(),
        reference_standard: None,
        reference_intakes: std::collections::BTreeMap::new(),
        allergen_summary: None,
    }
}

pub struct NutritionExtension {
    client: Arc<Client>,
    tracker: crate::checks::CheckTracker,
    failures: crate::failures::FailureTracker,
    client_profile: Option<Arc<crate::client::ClientProfile>>,
    matched_exclusions: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    unresolved_exclusions: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl NutritionExtension {
    pub fn new(client: Arc<Client>) -> Self {
        Self {
            client,
            tracker: crate::checks::CheckTracker::new(),
            failures: crate::failures::FailureTracker::new(),
            client_profile: None,
            matched_exclusions: Arc::new(std::sync::Mutex::new(Default::default())),
            unresolved_exclusions: Arc::new(std::sync::Mutex::new(Default::default())),
        }
    }

    /// Handle to the per-extension check tracker. Cloning is cheap (Arc).
    pub fn checks(&self) -> crate::checks::CheckTracker {
        self.tracker.clone()
    }

    /// Handle to the per-extension resolve-failure tracker. Cloning is cheap (Arc).
    pub fn failures(&self) -> crate::failures::FailureTracker {
        self.failures.clone()
    }

    pub fn with_client(mut self, profile: crate::client::ClientProfile) -> Self {
        self.client_profile = Some(Arc::new(profile));
        self
    }

    pub fn with_client_path(
        self,
        path: &std::path::Path,
    ) -> Result<Self, crate::client::ClientError> {
        let profile = crate::client::ClientProfile::from_path(path)?;
        Ok(self.with_client(profile))
    }

    fn exclusions_vec(&self) -> Vec<String> {
        self.client_profile
            .as_ref()
            .map(|p| p.exclusions.clone())
            .unwrap_or_default()
    }

    /// Serialised client profile for jinja context injection, or `None`
    /// if no profile is configured.
    pub fn client_context(&self) -> Option<serde_json::Value> {
        self.client_profile
            .as_ref()
            .map(|p| serde_json::to_value(&**p).expect("client profile serialises"))
    }
}

impl ConfigExtension for NutritionExtension {
    fn register(&self, env: &mut Environment<'_>) {
        const CK_MACROS: &str = include_str!("macros/ck.jinja");
        env.add_template("ck", CK_MACROS)
            .expect("ck.jinja macro library is invalid");

        // Shared allergen Contains-line partial. Templates render it with
        // `{% include "allergens" %}` after setting `alg` to an
        // `allergen_summary` value; the rendering contract is locked by
        // tests/allergen_template_test.rs against this same file.
        const ALLERGEN_PARTIAL: &str = include_str!("macros/allergens.jinja");
        env.add_template("allergens", ALLERGEN_PARTIAL)
            .expect("allergens.jinja partial is invalid");

        // Per-render reset: cooklang-reports calls register() each render.
        self.tracker.reset();
        self.failures.reset();
        self.matched_exclusions.lock().unwrap().clear();
        self.unresolved_exclusions.lock().unwrap().clear();

        let matched = self.matched_exclusions.clone();
        env.add_function(
            "matched_exclusions",
            move || -> Result<minijinja::Value, minijinja::Error> {
                let v: Vec<String> = matched.lock().unwrap().iter().cloned().collect();
                Ok(minijinja::Value::from_serialize(&v))
            },
        );
        let unresolved = self.unresolved_exclusions.clone();
        env.add_function(
            "unresolved_exclusions",
            move || -> Result<minijinja::Value, minijinja::Error> {
                let v: Vec<String> = unresolved.lock().unwrap().iter().cloned().collect();
                Ok(minijinja::Value::from_serialize(&v))
            },
        );

        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let matched = self.matched_exclusions.clone();
        let unresolved = self.unresolved_exclusions.clone();
        let failures = self.failures.clone();
        env.add_function(
            "nutrition_for",
            move |ing: minijinja::Value| -> Result<minijinja::Value, minijinja::Error> {
                let name = ing.get_attr("name")?.to_string();
                let quantity = ing.get_attr("quantity")?;
                let unit = quantity
                    .get_attr("unit")
                    .ok()
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                let value_str = quantity.get_attr("value")?.to_string();
                let amount = value_str.parse::<f64>().map_err(|_| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!("non-numeric quantity '{value_str}' not supported"),
                    )
                })?;
                let prep = ing
                    .get_attr("note")
                    .ok()
                    .map(|n| n.to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "raw".to_string());

                if exclusions.is_empty() {
                    // Fast path: no client profile or no exclusions configured.
                    let facts = client
                        .nutrition_for(&name, amount, &unit, &prep)
                        .map_err(|e| {
                            minijinja::Error::new(
                                minijinja::ErrorKind::InvalidOperation,
                                e.to_string(),
                            )
                        })?;
                    return Ok(minijinja::Value::from_serialize(&facts));
                }

                // Exclusion-aware path: wrap as a 1-item aggregate to pick up
                // alias-aware matching so ck.absent / matched_exclusions reflect this
                // ingredient even when the template only uses nutrition_for.
                let item = AggregateItem {
                    ingredient: name.clone(),
                    amount,
                    unit: unit.clone(),
                    prep: prep.clone(),
                    region: "us".into(),
                };
                let resp = client.aggregate(&[item], &exclusions, None).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                record_failures(&failures, &resp);
                {
                    let mut m = matched.lock().unwrap();
                    for x in &resp.matched_exclusions {
                        m.insert(x.clone());
                    }
                    let mut u = unresolved.lock().unwrap();
                    for x in &resp.unresolved_exclusions {
                        u.insert(x.clone());
                    }
                }
                let facts = if let Some(it) = resp.items.first() {
                    NutritionFacts {
                        kcal: it.macros.kcal,
                        protein_g: it.macros.protein_g,
                        fat_g: it.macros.fat_g,
                        carb_g: it.macros.carb_g,
                        fiber_g: it.macros.fiber_g,
                    }
                } else if let Some(failure) = resp.failures.first() {
                    return Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!(
                            "nutrition_for failed for '{}': {}",
                            name, failure.error.message
                        ),
                    ));
                } else {
                    return Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!("nutrition_for returned no items for '{name}'"),
                    ));
                };
                Ok(minijinja::Value::from_serialize(&facts))
            },
        );
        env.add_function("compare", compare_fn);
        env.add_function("within_tol", within_tol);

        let tracker = self.tracker.clone();
        env.add_function(
            "record_check",
            move |label: String, ok: bool| -> Result<minijinja::Value, minijinja::Error> {
                tracker.record(label, ok);
                Ok(minijinja::Value::from(""))
            },
        );

        let tracker = self.tracker.clone();
        env.add_function(
            "failed_checks",
            move || -> Result<minijinja::Value, minijinja::Error> {
                let recs: Vec<_> = tracker.snapshot().into_iter().filter(|c| !c.ok).collect();
                Ok(minijinja::Value::from_serialize(&recs))
            },
        );

        let tracker = self.tracker.clone();
        env.add_function(
            "all_checks",
            move || -> Result<minijinja::Value, minijinja::Error> {
                Ok(minijinja::Value::from_serialize(tracker.snapshot()))
            },
        );

        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let matched = self.matched_exclusions.clone();
        let unresolved = self.unresolved_exclusions.clone();
        let failures = self.failures.clone();
        env.add_function(
            "aggregate_nutrition",
            move |ingredients: minijinja::Value,
                  standard: Option<String>|
                  -> Result<minijinja::Value, minijinja::Error> {
                let items = ingredients_to_items(&ingredients)?;
                if items.is_empty() {
                    return Ok(minijinja::Value::from_serialize(empty_aggregate_response()));
                }
                // Optional reference standard (`fda`, `eu`, `uk`); it also selects
                // the allergen view. Empty or absent keeps the service default.
                let std_slug = standard.filter(|s| !s.is_empty());
                let resp = client
                    .aggregate(&items, &exclusions, std_slug.as_deref())
                    .map_err(|e| {
                        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                    })?;
                record_failures(&failures, &resp);
                {
                    let mut m = matched.lock().unwrap();
                    for x in &resp.matched_exclusions {
                        m.insert(x.clone());
                    }
                    let mut u = unresolved.lock().unwrap();
                    for x in &resp.unresolved_exclusions {
                        u.insert(x.clone());
                    }
                }
                Ok(minijinja::Value::from_serialize(&resp))
            },
        );

        // nutrition_report(ingredients) — like aggregate_nutrition, but returns a
        // serialized `RecipeReport` JSON string keeping per-failure codes + the
        // units we actually sent (corpus harness needs these; the plain aggregate
        // shape throws them away). Does not touch the exclusion trackers — report
        // mode doesn't need them.
        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let failures = self.failures.clone();
        env.add_function(
            "nutrition_report",
            move |ingredients: minijinja::Value| -> Result<minijinja::Value, minijinja::Error> {
                // Raw list length = the resolve-rate denominator (matches the old
                // `il | length` metric, which includes references).
                let listed = ingredients.try_iter()?.count();
                let items = ingredients_to_items(&ingredients)?;
                let resp = if items.is_empty() {
                    empty_aggregate_response()
                } else {
                    client.aggregate(&items, &exclusions, None).map_err(|e| {
                        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                    })?
                };
                record_failures(&failures, &resp);
                let report = crate::report::build_recipe_report(listed, &items, &resp);
                let json = serde_json::to_string(&report).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                Ok(minijinja::Value::from(json))
            },
        );

        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let matched = self.matched_exclusions.clone();
        let unresolved = self.unresolved_exclusions.clone();
        let failures = self.failures.clone();
        env.add_function(
            "total_calories",
            move |ingredients: minijinja::Value| -> Result<f64, minijinja::Error> {
                let items = ingredients_to_items(&ingredients)?;
                if items.is_empty() {
                    return Ok(0.0);
                }
                let resp = client.aggregate(&items, &exclusions, None).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                record_failures(&failures, &resp);
                {
                    let mut m = matched.lock().unwrap();
                    for x in &resp.matched_exclusions {
                        m.insert(x.clone());
                    }
                    let mut u = unresolved.lock().unwrap();
                    for x in &resp.unresolved_exclusions {
                        u.insert(x.clone());
                    }
                }
                Ok(resp.totals.macros.kcal)
            },
        );

        // "macros" is the spec-defined name (§2). In Jinja2 "macros" is a keyword,
        // but minijinja functions are registered separately from template macros so
        // there is no actual conflict; user template macros can still be defined.
        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let matched = self.matched_exclusions.clone();
        let unresolved = self.unresolved_exclusions.clone();
        let failures = self.failures.clone();
        env.add_function(
            "macros",
            move |ingredients: minijinja::Value| -> Result<minijinja::Value, minijinja::Error> {
                let items = ingredients_to_items(&ingredients)?;
                if items.is_empty() {
                    return Ok(minijinja::Value::from_serialize(
                        &empty_aggregate_response().totals.macros,
                    ));
                }
                let resp = client.aggregate(&items, &exclusions, None).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                record_failures(&failures, &resp);
                {
                    let mut m = matched.lock().unwrap();
                    for x in &resp.matched_exclusions {
                        m.insert(x.clone());
                    }
                    let mut u = unresolved.lock().unwrap();
                    for x in &resp.unresolved_exclusions {
                        u.insert(x.clone());
                    }
                }
                Ok(minijinja::Value::from_serialize(&resp.totals.macros))
            },
        );

        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let matched = self.matched_exclusions.clone();
        let unresolved = self.unresolved_exclusions.clone();
        let failures = self.failures.clone();
        env.add_function(
            "nutrient_total",
            move |ingredients: minijinja::Value, name: String| -> Result<f64, minijinja::Error> {
                let items = ingredients_to_items(&ingredients)?;
                if items.is_empty() {
                    return Ok(0.0);
                }
                let resp = client.aggregate(&items, &exclusions, None).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                record_failures(&failures, &resp);
                {
                    let mut m = matched.lock().unwrap();
                    for x in &resp.matched_exclusions {
                        m.insert(x.clone());
                    }
                    let mut u = unresolved.lock().unwrap();
                    for x in &resp.unresolved_exclusions {
                        u.insert(x.clone());
                    }
                }
                // Searches micros first, then vitamins, so callers can request
                // either bucket by key (e.g. "iron_mg" or "vit_d_iu").
                Ok(resp
                    .totals
                    .micros
                    .get(&name)
                    .or_else(|| resp.totals.vitamins.get(&name))
                    .copied()
                    .unwrap_or(0.0))
            },
        );

        // vitamins(ingredients) — totals.vitamins shortcut (spec §7.1).
        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let failures = self.failures.clone();
        env.add_function(
            "vitamins",
            move |ingredients: minijinja::Value| -> Result<minijinja::Value, minijinja::Error> {
                let items = ingredients_to_items(&ingredients)?;
                if items.is_empty() {
                    return Ok(minijinja::Value::from_serialize(
                        &empty_aggregate_response().totals.vitamins,
                    ));
                }
                let resp = client.aggregate(&items, &exclusions, None).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                record_failures(&failures, &resp);
                Ok(minijinja::Value::from_serialize(&resp.totals.vitamins))
            },
        );

        // nutrition_for_amount(name, amount, unit, prep) — lower-level variant
        // of nutrition_for when you don't have an ingredient object (spec §7.1).
        // Returns the full per-item shape (macros + micros + vitamins +
        // source + confidence) via a 1-item aggregate so exclusions still apply.
        let client = self.client.clone();
        let exclusions = self.exclusions_vec();
        let failures = self.failures.clone();
        env.add_function(
            "nutrition_for_amount",
            move |name: String,
                  amount: f64,
                  unit: Option<String>,
                  prep: Option<String>,
                  standard: Option<String>|
                  -> Result<minijinja::Value, minijinja::Error> {
                let item = AggregateItem {
                    ingredient: name.clone(),
                    amount,
                    unit: unit.unwrap_or_default(),
                    prep: prep
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "raw".into()),
                    region: "us".into(),
                };
                let std_slug = standard.filter(|s| !s.is_empty());
                let resp = client
                    .aggregate(&[item], &exclusions, std_slug.as_deref())
                    .map_err(|e| {
                        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                    })?;
                record_failures(&failures, &resp);
                if let Some(it) = resp.items.first() {
                    Ok(minijinja::Value::from_serialize(it))
                } else if let Some(failure) = resp.failures.first() {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!(
                            "nutrition_for_amount failed for '{}': {}",
                            name, failure.error.message
                        ),
                    ))
                } else {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!("nutrition_for_amount returned no items for '{name}'"),
                    ))
                }
            },
        );

        // is_in_category(ingredient, slug) — taxonomy check (spec §7.1).
        // Accepts either an ingredient object (with .name) or a bare string.
        let client = self.client.clone();
        env.add_function(
            "is_in_category",
            move |ingredient: minijinja::Value, slug: String| -> Result<bool, minijinja::Error> {
                let name = value_to_ingredient_name(&ingredient)?;
                client.is_in_category(&name, &slug).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })
            },
        );

        // category_servings(plan, slug) — number of meals containing at least
        // one ingredient in the category (spec §7.1). Memoises membership per
        // ingredient name to bound HTTP calls.
        let client = self.client.clone();
        env.add_function(
            "category_servings",
            move |plan: minijinja::Value, slug: String| -> Result<i64, minijinja::Error> {
                let mut memo: std::collections::HashMap<String, bool> =
                    std::collections::HashMap::new();
                let mut count: i64 = 0;
                let days = plan.get_attr("days").unwrap_or(minijinja::Value::UNDEFINED);
                for day in days.try_iter().into_iter().flatten() {
                    let meals = day.get_attr("meals").unwrap_or(minijinja::Value::UNDEFINED);
                    for meal in meals.try_iter().into_iter().flatten() {
                        let mut meal_has_member = false;
                        let recipes = meal
                            .get_attr("recipes")
                            .unwrap_or(minijinja::Value::UNDEFINED);
                        'recipes: for recipe in recipes.try_iter().into_iter().flatten() {
                            let ings = recipe
                                .get_attr("ingredients")
                                .unwrap_or(minijinja::Value::UNDEFINED);
                            for ing in ings.try_iter().into_iter().flatten() {
                                let Ok(name) = value_to_ingredient_name(&ing) else {
                                    continue;
                                };
                                let member = if let Some(m) = memo.get(&name) {
                                    *m
                                } else {
                                    let m = client.is_in_category(&name, &slug).map_err(|e| {
                                        minijinja::Error::new(
                                            minijinja::ErrorKind::InvalidOperation,
                                            e.to_string(),
                                        )
                                    })?;
                                    memo.insert(name.clone(), m);
                                    m
                                };
                                if member {
                                    meal_has_member = true;
                                    break 'recipes;
                                }
                            }
                        }
                        if meal_has_member {
                            count += 1;
                        }
                    }
                }
                Ok(count)
            },
        );

        // convert(amount, from, to, ingredient?) — unit conversion (spec §7.1).
        // ingredient may be omitted (mass↔mass) or be a string / ingredient obj.
        let client = self.client.clone();
        env.add_function(
            "convert",
            move |amount: f64,
                  from: String,
                  to: String,
                  ingredient: Option<minijinja::Value>|
                  -> Result<f64, minijinja::Error> {
                let name = match ingredient {
                    Some(v) if !v.is_none() && !v.is_undefined() => {
                        Some(value_to_ingredient_name(&v)?)
                    }
                    _ => None,
                };
                client
                    .convert(amount, &from, &to, name.as_deref(), None, Some("us"))
                    .map_err(|e| {
                        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                    })
            },
        );

        // reference_intake(name, standard?) — look up a single nutrient's daily
        // reference value. Returns undefined when the nutrient is not in the table
        // (so `{{ reference_intake('no_such') is undefined }}` is true in templates).
        let client = self.client.clone();
        env.add_function(
            "reference_intake",
            move |name: String,
                  standard: Option<String>|
                  -> Result<minijinja::Value, minijinja::Error> {
                let std_slug = standard
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "fda".into());
                let table = client.reference_intakes(&std_slug).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                Ok(match table.get(&name) {
                    Some(v) => minijinja::Value::from(*v),
                    None => minijinja::Value::UNDEFINED,
                })
            },
        );

        // dv_percent(name, amount, standard?) — express `amount` as a percentage
        // of the daily reference value for `name`. Returns undefined (not a
        // divide-by-zero) when the nutrient has no DV or its DV is zero.
        let client = self.client.clone();
        env.add_function(
            "dv_percent",
            move |name: String,
                  amount: f64,
                  standard: Option<String>|
                  -> Result<minijinja::Value, minijinja::Error> {
                let std_slug = standard
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "fda".into());
                let table = client.reference_intakes(&std_slug).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                })?;
                Ok(match table.get(&name) {
                    Some(r) if *r > 0.0 => minijinja::Value::from(amount / r * 100.0),
                    _ => minijinja::Value::UNDEFINED,
                })
            },
        );
    }
}

/// Extract an ingredient name from a jinja value: a bare string, or an object
/// exposing a `.name` attribute (the cooklang-reports ingredient shape).
fn value_to_ingredient_name(v: &minijinja::Value) -> Result<String, minijinja::Error> {
    if let Some(s) = v.as_str() {
        return Ok(s.to_string());
    }
    match v.get_attr("name") {
        Ok(n) if !n.is_undefined() && !n.is_none() => Ok(n.to_string()),
        _ => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "expected an ingredient name (string) or an object with a .name attribute",
        )),
    }
}

#[cfg(test)]
mod amount_tests {
    use super::interpret_amount;

    fn amount(value: &str, unit: &str) -> (f64, String) {
        interpret_amount(value, unit)
    }

    fn assert_amount(value: &str, expected: f64) {
        let (n, u) = amount(value, "cup");
        assert!(
            (n - expected).abs() < 1e-9,
            "{value:?}: expected {expected}, got {n}"
        );
        assert_eq!(u, "cup", "{value:?}: unit changed");
    }

    #[test]
    fn unicode_vulgar_fractions() {
        let cases = [
            ("½", 0.5),
            ("¼", 0.25),
            ("¾", 0.75),
            ("⅓", 1.0 / 3.0),
            ("⅔", 2.0 / 3.0),
            ("⅛", 0.125),
            ("⅜", 0.375),
            ("⅝", 0.625),
            ("⅞", 0.875),
            ("⅕", 0.2),
            ("⅖", 0.4),
            ("⅗", 0.6),
            ("⅘", 0.8),
            ("⅙", 1.0 / 6.0),
            ("⅚", 5.0 / 6.0),
        ];
        for (v, e) in cases {
            assert_amount(v, e);
        }
    }

    #[test]
    fn fraction_ranges_use_lower_bound() {
        assert_amount("3/4-1", 0.75);
        assert_amount("1/2 - 3/4", 0.5);
        assert_amount("½-¾", 0.5);
    }

    #[test]
    fn mixed_numbers() {
        assert_amount("1 and 1/2", 1.5);
        assert_amount("1 ½", 1.5);
        assert_amount("1½", 1.5);
        assert_amount("1 1/2", 1.5);
        assert_amount("2 and ½", 2.5);
    }

    #[test]
    fn numeric_ranges_use_lower_bound() {
        assert_amount("1-2", 1.0);
        assert_amount("2-3", 2.0);
        assert_amount("2-2.25", 2.0);
        assert_amount("1 to 2", 1.0);
        assert_amount("1–2", 1.0);
        assert_amount("1—2", 1.0);
        assert_amount("1 - 2", 1.0);
    }

    #[test]
    fn plain_fraction_text() {
        assert_amount("1/2", 0.5);
        assert_amount("3/4", 0.75);
    }

    #[test]
    fn plain_numbers() {
        assert_amount("2", 2.0);
        assert_amount("0.5", 0.5);
        assert_amount(" 150 ", 150.0);
        assert_amount("2 large", 2.0);
    }

    #[test]
    fn zero_denominator_is_not_a_fraction() {
        // `1/0` is not a fraction; the leading `1` still stands (as `1-2` does).
        assert_amount("1/0", 1.0);
        assert_eq!(amount("0/0", "cup"), (1.0, "cup".to_string()));
    }

    #[test]
    fn text_mappings_preserved() {
        for v in ["pinch", "a pinch", "Pinches"] {
            assert_eq!(amount(v, "g"), (1.0, "pinch".to_string()), "{v}");
        }
        assert_eq!(amount("dash", ""), (1.0, "dash".to_string()));
        for v in [
            "to taste",
            "as needed",
            "as required",
            "to serve",
            "for serving",
            "for garnish",
            "to garnish",
            "for dusting",
            "optional",
        ] {
            assert_eq!(amount(v, "g"), (1.0, "pinch".to_string()), "{v}");
        }
    }

    #[test]
    fn empty_is_pinch() {
        assert_eq!(amount("", ""), (1.0, "pinch".to_string()));
        assert_eq!(amount("   ", ""), (1.0, "pinch".to_string()));
    }

    #[test]
    fn unrecognised_text_fallbacks() {
        assert_eq!(amount("some", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("some", ""), (1.0, "pinch".to_string()));
        assert_eq!(amount("and 1/2", "cup"), (1.0, "cup".to_string()));
    }

    #[test]
    fn non_positive_and_nan_not_accepted() {
        assert_eq!(amount("0", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("-1", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("NaN", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("inf", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("0/4", "cup"), (1.0, "cup".to_string()));
        assert_eq!(amount("0", ""), (1.0, "pinch".to_string()));
    }
}
