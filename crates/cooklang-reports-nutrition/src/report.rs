//! Structured per-recipe corpus report. The jinja `nutrition_report` function
//! is a thin wrapper around `build_recipe_report`; the correlation logic lives
//! here so it is testable without a live service.

use cookmd_nutrition_client::{AggregateItem, AggregateResponse};
use serde::Serialize;

/// One failed ingredient, enriched with the unit/amount we actually sent (the
/// service's failure object omits them). `unit` is empty for a bare count.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FailureDetail {
    pub index: usize,
    pub code: String,
    pub ingredient: String,
    pub unit: String,
    pub amount: f64,
}

/// One recipe's resolution outcome, one JSON row in the corpus report.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecipeReport {
    pub ingredients_listed: usize,
    pub ingredients_resolved: usize,
    pub ingredients_failed: usize,
    pub failures: Vec<FailureDetail>,
}

/// Correlate each `AggregateFailure` back to the item we sent (by `index`) so the
/// failure carries the unit/amount. `listed` is the raw ingredient-list length
/// (NOT `items.len()`), to keep the resolve-rate denominator identical to the
/// previous `corpus-eval.json.jinja` metric, which counted `il | length`.
pub fn build_recipe_report(
    listed: usize,
    items: &[AggregateItem],
    resp: &AggregateResponse,
) -> RecipeReport {
    let failures = resp
        .failures
        .iter()
        .map(|f| {
            let sent = items.get(f.index);
            FailureDetail {
                index: f.index,
                code: f.error.code.clone(),
                ingredient: f.ingredient.clone(),
                unit: sent.map(|i| i.unit.clone()).unwrap_or_default(),
                amount: sent.map(|i| i.amount).unwrap_or(0.0),
            }
        })
        .collect();
    RecipeReport {
        ingredients_listed: listed,
        ingredients_resolved: resp.items.len(),
        ingredients_failed: resp.failures.len(),
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cookmd_nutrition_client::{
        AggregateFailure, AggregateFailureError, ConfidenceBreakdown, MacroTotals, Totals,
    };
    // `AggregateItem` and `AggregateResponse` come in via `use super::*;`.

    fn item(ingredient: &str, amount: f64, unit: &str) -> AggregateItem {
        AggregateItem {
            ingredient: ingredient.into(),
            amount,
            unit: unit.into(),
            prep: String::new(),
            region: "us".into(),
        }
    }

    fn resp_with_failures(n_items: usize, failures: Vec<AggregateFailure>) -> AggregateResponse {
        AggregateResponse {
            items: (0..n_items)
                .map(|_| serde_json::from_value(serde_json::json!({
                    "ingredient": "x", "preparation": "raw",
                    "amount": {"value": 1.0, "unit": "g", "mass_g": 1.0},
                    "macros": {"kcal":0.0,"protein_g":0.0,"fat_g":0.0,"carb_g":0.0,"fiber_g":0.0,"sugar_g":0.0,"sat_fat_g":0.0},
                    "micros": {}, "vitamins": {}, "source": "usda",
                    "confidence": "confirmed", "warnings": []
                })).unwrap())
                .collect(),
            failures,
            totals: Totals {
                mass_g: 0.0,
                macros: MacroTotals { kcal:0.0,protein_g:0.0,fat_g:0.0,carb_g:0.0,fiber_g:0.0,sugar_g:0.0,sat_fat_g:0.0 },
                micros: Default::default(), vitamins: Default::default(),
                confidence: "confirmed".into(), confidence_weighted: "confirmed".into(),
                is_partial: false, included_count: n_items, failed_count: 0,
            },
            confidence_breakdown: ConfidenceBreakdown {
                confirmed_items: n_items, partial_items: 0, estimated_items: 0,
                estimated_ingredients: vec![], estimated_share_of_micronutrients: None,
            },
            matched_exclusions: vec![],
            unresolved_exclusions: vec![],
            reference_standard: None,
            reference_intakes: std::collections::BTreeMap::new(),
            allergen_summary: None,
        }
    }

    fn fail(index: usize, ingredient: &str, code: &str) -> AggregateFailure {
        AggregateFailure {
            index,
            ingredient: ingredient.into(),
            error: AggregateFailureError {
                code: code.into(),
                message: "m".into(),
                field: None,
                received: None,
                suggestions: Vec::new(),
            },
        }
    }

    #[test]
    fn correlates_failure_index_to_sent_unit() {
        // Sent 3 items; item at index 1 failed with density_unavailable using "cup".
        let items = vec![
            item("salmon", 150.0, "g"),
            item("ginger", 1.0, "cup"),
            item("star fruit", 1.0, ""),
        ];
        let resp = resp_with_failures(
            1,
            vec![
                fail(1, "ginger", "density_unavailable"),
                fail(2, "star fruit", "ingredient_not_found"),
            ],
        );

        let report = build_recipe_report(3, &items, &resp);

        assert_eq!(report.ingredients_listed, 3);
        assert_eq!(report.ingredients_resolved, 1);
        assert_eq!(report.ingredients_failed, 2);
        assert_eq!(
            report.failures[0],
            FailureDetail {
                index: 1,
                code: "density_unavailable".into(),
                ingredient: "ginger".into(),
                unit: "cup".into(),
                amount: 1.0,
            }
        );
        // Bare-count failure echoes empty unit.
        assert_eq!(report.failures[1].unit, "");
        assert_eq!(report.failures[1].code, "ingredient_not_found");
    }

    #[test]
    fn out_of_range_index_falls_back_to_empty_unit() {
        // Defensive: a failure index that doesn't map to a sent item must not panic.
        let items = vec![item("salt", 1.0, "pinch")];
        let resp = resp_with_failures(0, vec![fail(9, "salt", "ingredient_not_found")]);
        let report = build_recipe_report(1, &items, &resp);
        assert_eq!(report.failures[0].unit, "");
        assert_eq!(report.failures[0].amount, 0.0);
    }
}
