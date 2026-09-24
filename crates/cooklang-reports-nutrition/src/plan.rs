//! Plan support for .menu files: data shape + builder.

use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

static DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{4}-\d{2}-\d{2})\)").unwrap());
static TIME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{2}:\d{2})\)").unwrap());
static MEAL_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\(\d{2}:\d{2}\)\s*").unwrap());

pub fn extract_date(s: &str) -> Option<String> {
    DATE_RE.captures(s).map(|c| c[1].to_string())
}

pub fn extract_meal_time(header: &str) -> Option<String> {
    TIME_RE.captures(header).map(|c| c[1].to_string())
}

pub fn extract_meal_type(header: &str) -> String {
    let stripped = header.trim().trim_end_matches(':').trim();
    MEAL_HEADER_RE.replace_all(stripped, "").trim().to_string()
}

pub fn is_meal_header(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.ends_with(':') {
        return false;
    }
    let before = trimmed.trim_end_matches(':').trim();
    !before.is_empty()
}

/// Detect a meal header inside a step's text item, returning the header with
/// list glue stripped. Menus commonly put the recipe ref on the next line as a
/// bullet (`Breakfast:\n- @./ref{}`); cooklang collapses the newline so the
/// text arrives as `"Breakfast: - "` — strip trailing `-`/whitespace before
/// the ends-with-`:` check. A bare separator between two refs (`" - "`)
/// strips to nothing and is not a header.
pub fn meal_header_line(line: &str) -> Option<&str> {
    let stripped = line.trim().trim_end_matches(['-', ' ', '\t']);
    if is_meal_header(stripped) {
        Some(stripped)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Plan {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub days: Vec<PlanDay>,
    /// Sum of `meals.len()` across all days. Meals with no recipe references
    /// (e.g., a `Snacks:` header followed by nothing) still count.
    pub total_meal_count: usize,
    pub unique_recipe_count: usize,
    pub all_ingredients: Vec<ScaledIngredient>,
    /// Recipe references that could not be expanded (file missing, parse
    /// failure, recursion). Their ingredients are absent from every total —
    /// templates must surface these or the report silently undercounts.
    pub missing_recipes: Vec<MissingRecipe>,
    /// `servings:` from the menu frontmatter (people served), when numeric.
    /// Lets templates compute per-person figures from plan totals.
    pub servings: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MissingRecipe {
    pub name: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PlanDay {
    pub date: Option<String>,
    pub name: Option<String>,
    pub meals: Vec<PlanMeal>,
    /// Flattened ingredients for this day: everything from
    /// `meals[].recipes[].ingredients` plus loose menu ingredients
    /// (`@apples{5}` outside any recipe ref). Mirrors `Plan::all_ingredients`
    /// but scoped to one day, so weekly templates can do per-day macro checks
    /// via `macros(day.ingredients)`.
    pub ingredients: Vec<ScaledIngredient>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PlanMeal {
    pub meal_type: String,
    pub time: Option<String>,
    pub recipes: Vec<PlanRecipe>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PlanRecipe {
    pub name: String,
    /// Factor actually applied to the recipe. For `{3%servings}` against a
    /// 6-serving recipe this is 0.5, not 3.
    pub scale: f64,
    /// Unit of the reference quantity (`servings`, a yield unit, or `None` for
    /// a bare multiplier). Internal to resolution.
    #[serde(skip)]
    pub scale_unit: Option<String>,
    pub ingredients: Vec<ScaledIngredient>,
    /// Directory components of the recipe reference (`@./Salads/Caprese{}` →
    /// `[".", "Salads"]`), resolved against the plan's base path. Internal to
    /// resolution — not part of the plan JSON surface.
    #[serde(skip)]
    pub components: Vec<String>,
}

/// Shape matches what `ingredients_to_items()` in lib.rs already consumes
/// (nested `.quantity.value / .quantity.unit / .note`), so
/// `aggregate_nutrition(plan.all_ingredients)` works without a second adapter.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ScaledIngredient {
    pub name: String,
    pub quantity: Option<ScaledQuantity>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScaledQuantity {
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("recipe not found: {ref_path}")]
    RecipeNotFound { ref_path: PathBuf },
    #[error("recursive recipe reference: {ref_path}")]
    RecursiveRef { ref_path: PathBuf },
    #[error("failed to read {ref_path}: {source}")]
    ReadFailed {
        ref_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {ref_path}: {message}")]
    ParseFailed { ref_path: PathBuf, message: String },
}

use cooklang::{
    Content, Converter, CooklangParser, Extensions, Ingredient as CookIngredient, Item, Recipe,
};
use std::path::Path;

/// Parse a `.menu` source string and build the plan.
///
/// `menu_path` is reported in `PlanError::ParseFailed` so users see the actual
/// file name; pass `None` for in-memory sources without a path.
pub fn build_plan_from_source(
    menu_src: &str,
    base_path: &Path,
    menu_path: Option<&Path>,
) -> Result<Plan, PlanError> {
    let parser = CooklangParser::new(Extensions::all(), Converter::default());
    let (mut recipe, _warnings) =
        parser
            .parse(menu_src)
            .into_result()
            .map_err(|errors| PlanError::ParseFailed {
                ref_path: menu_path.unwrap_or(base_path).to_path_buf(),
                message: format!("{errors}"),
            })?;
    // `.menu` files don't get user-driven scaling; pass 1.0 so quantities stay as written.
    recipe.scale(1.0, &Converter::default());
    build_plan(&recipe, base_path)
}

/// Build a [`Plan`] from a parsed, scaled `.menu` recipe.
pub fn build_plan(recipe: &Recipe, base_path: &Path) -> Result<Plan, PlanError> {
    let mut plan = Plan {
        servings: recipe.metadata.servings().and_then(|s| match s {
            cooklang::metadata::Servings::Number(n) => Some(n),
            cooklang::metadata::Servings::Text(_) => None,
        }),
        ..Plan::default()
    };
    let mut unique: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for section in &recipe.sections {
        let date = section.name.as_deref().and_then(extract_date);
        let mut day = PlanDay {
            date,
            name: section.name.clone(),
            meals: Vec::new(),
            ingredients: Vec::new(),
        };

        // Walking convention: text items that look like meal headers open a new
        // PlanMeal. Recipe-ref ingredients attach to the most recent meal, or
        // to an unnamed fallback meal if none has opened.
        let mut current: Option<PlanMeal> = None;

        for content in &section.content {
            let Content::Step(step) = content else {
                continue;
            };
            for item in &step.items {
                match item {
                    Item::Text { value } => {
                        for line in value.split('\n') {
                            if let Some(header) = meal_header_line(line) {
                                if let Some(meal) = current.take() {
                                    day.meals.push(meal);
                                }
                                current = Some(PlanMeal {
                                    meal_type: extract_meal_type(header),
                                    time: extract_meal_time(header),
                                    recipes: Vec::new(),
                                });
                            }
                        }
                    }
                    Item::Ingredient { index } => {
                        let Some(ing) = recipe.ingredients.get(*index) else {
                            continue;
                        };
                        let Some(reference) = ing.reference.as_ref() else {
                            // Loose menu ingredient (`@apples{5}` in a Snacks
                            // section) — counts toward the day like any
                            // recipe-expanded ingredient.
                            day.ingredients.push(ScaledIngredient {
                                name: ing.name.clone(),
                                quantity: ing.quantity.as_ref().and_then(quantity_to_scaled),
                                note: ing.note.clone(),
                            });
                            continue;
                        };
                        let (scale, scale_unit) = ingredient_scale_target(ing);
                        let name = reference_stem(reference);
                        unique.insert(name.clone());
                        let meal = current.get_or_insert_with(PlanMeal::default);
                        meal.recipes.push(PlanRecipe {
                            name,
                            scale,
                            scale_unit,
                            ingredients: Vec::new(),
                            components: reference.components.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
        if let Some(meal) = current.take() {
            day.meals.push(meal);
        }
        plan.total_meal_count += day.meals.len();
        plan.days.push(day);
    }

    let mut in_progress: HashSet<PathBuf> = HashSet::new();
    let mut missing: Vec<MissingRecipe> = Vec::new();
    for day in &mut plan.days {
        for meal in &mut day.meals {
            for r in &mut meal.recipes {
                // Top-level refs resolve against the plan's base path with their
                // directory components intact (`@./Salads/Caprese` →
                // `<base>/Salads/Caprese.cook`); `.`/`..` segments collapse in
                // load_recipe_ingredients' canonicalize.
                let reference = cooklang::RecipeReference {
                    name: r.name.clone(),
                    components: r.components.clone(),
                };
                let p = resolve_ref_path(base_path, &reference);
                // A recipe that can't be expanded (missing file, parse error,
                // recursion) contributes nothing; record it instead of failing
                // the whole plan, so one dangling ref doesn't kill the report.
                match load_recipe_ingredients(
                    &p,
                    r.scale,
                    r.scale_unit.as_deref(),
                    &mut in_progress,
                ) {
                    Ok((ings, factor)) => {
                        r.ingredients = ings;
                        r.scale = factor;
                    }
                    Err(e) => {
                        missing.push(MissingRecipe {
                            name: r.name.clone(),
                            error: e.to_string(),
                        });
                        continue;
                    }
                }
                day.ingredients.extend(r.ingredients.iter().cloned());
            }
        }
        plan.all_ingredients.extend(day.ingredients.iter().cloned());
    }

    let mut dates: Vec<&String> = plan.days.iter().filter_map(|d| d.date.as_ref()).collect();
    dates.sort();
    plan.start_date = dates.first().map(|s| (*s).clone());
    plan.end_date = dates.last().map(|s| (*s).clone());
    plan.unique_recipe_count = unique.len();
    plan.missing_recipes = missing;
    Ok(plan)
}

/// Extract the scaling target from a recipe reference's quantity: the number
/// plus its unit (`{3%servings}` → `(3.0, Some("servings"))`, `{2}` →
/// `(2.0, None)`). Missing or non-numeric quantities (text/range) scale by 1.0.
fn ingredient_scale_target(ing: &CookIngredient) -> (f64, Option<String>) {
    let Some(q) = ing.quantity.as_ref() else {
        return (1.0, None);
    };
    // `q.value()` returns `&Value`; destructure with `&` so `n: Number` (Copy)
    // and we can call `value(self)`.
    if let &cooklang::Value::Number(n) = q.value() {
        (n.value(), q.unit().map(str::to_string))
    } else {
        (1.0, None)
    }
}

/// Scale `recipe` to the reference target and return the factor applied.
/// `{3%servings}` scales *to* 3 servings (×0.5 for a 6-serving recipe). A bare
/// number, any other unit, or a recipe without numeric `servings:` falls back
/// to a raw multiplier.
fn scale_recipe_to_target(recipe: &mut Recipe, value: f64, unit: Option<&str>) -> f64 {
    let base = recipe
        .metadata
        .servings()
        .and_then(|s| s.as_number())
        .filter(|&b| b > 0);
    let factor = match (unit, base) {
        (Some("servings" | "serving"), Some(base)) => value / base as f64,
        _ => value,
    };
    recipe.scale(factor, &Converter::default());
    factor
}

fn reference_stem(r: &cooklang::RecipeReference) -> String {
    // The ref's `name` may itself include path bits; the unique key is the
    // basename without extension.
    std::path::Path::new(&r.name)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| r.name.clone())
}

fn resolve_ref_path(base_path: &Path, reference: &cooklang::RecipeReference) -> PathBuf {
    let mut p = base_path.to_path_buf();
    for c in &reference.components {
        p.push(c);
    }
    p.push(&reference.name);
    p.set_extension("cook");
    p
}

fn quantity_to_scaled(q: &cooklang::Quantity) -> Option<ScaledQuantity> {
    let value = if let &cooklang::Value::Number(n) = q.value() {
        n.value()
    } else {
        return None;
    };
    let unit = q.unit().unwrap_or("").to_string();
    Some(ScaledQuantity { value, unit })
}

fn load_recipe_ingredients(
    ref_path: &Path,
    scale: f64,
    scale_unit: Option<&str>,
    in_progress: &mut HashSet<PathBuf>,
) -> Result<(Vec<ScaledIngredient>, f64), PlanError> {
    let canonical = ref_path.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PlanError::RecipeNotFound {
            ref_path: ref_path.to_path_buf(),
        },
        _ => PlanError::ReadFailed {
            ref_path: ref_path.to_path_buf(),
            source: e,
        },
    })?;
    if !in_progress.insert(canonical.clone()) {
        return Err(PlanError::RecursiveRef {
            ref_path: canonical,
        });
    }
    let src = std::fs::read_to_string(&canonical).map_err(|e| PlanError::ReadFailed {
        ref_path: canonical.clone(),
        source: e,
    })?;
    let parser = CooklangParser::new(Extensions::all(), Converter::default());
    let (mut recipe, _w) =
        parser
            .parse(&src)
            .into_result()
            .map_err(|errs| PlanError::ParseFailed {
                ref_path: canonical.clone(),
                message: format!("{errs}"),
            })?;
    let factor = scale_recipe_to_target(&mut recipe, scale, scale_unit);

    let mut out = Vec::new();
    for ing in &recipe.ingredients {
        if let Some(inner_ref) = ing.reference.as_ref() {
            // Nested ref resolves relative to *this* recipe's directory.
            // The quantity was already multiplied by this recipe's factor, so
            // `{3%servings}` here arrives as "3 × factor servings".
            let (inner_scale, inner_unit) = ingredient_scale_target(ing);
            let inner_base = canonical
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            let inner_path = resolve_ref_path(&inner_base, inner_ref);
            let (nested, _) = load_recipe_ingredients(
                &inner_path,
                inner_scale,
                inner_unit.as_deref(),
                in_progress,
            )?;
            out.extend(nested);
        } else {
            out.push(ScaledIngredient {
                name: ing.name.clone(),
                quantity: ing.quantity.as_ref().and_then(quantity_to_scaled),
                note: ing.note.clone(),
            });
        }
    }
    in_progress.remove(&canonical);
    Ok((out, factor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn build_plan_empty_menu_returns_empty_plan() {
        // An empty cooklang file parses to a recipe with no sections.
        let menu_src = "---\ntitle: Empty\n---\n";
        let plan = build_plan_from_source(menu_src, Path::new("."), None).unwrap();
        assert!(plan.days.is_empty());
        assert_eq!(plan.total_meal_count, 0);
        assert_eq!(plan.unique_recipe_count, 0);
        assert!(plan.all_ingredients.is_empty());
        assert!(plan.start_date.is_none());
        assert!(plan.end_date.is_none());
    }

    #[test]
    fn extract_date_finds_iso_in_parens() {
        assert_eq!(
            extract_date("Day 1 (2026-03-04)"),
            Some("2026-03-04".to_string())
        );
        assert_eq!(
            extract_date("(2026-12-31) Year end"),
            Some("2026-12-31".to_string())
        );
    }

    #[test]
    fn extract_date_returns_none_when_absent() {
        assert_eq!(extract_date("Day 1"), None);
        assert_eq!(extract_date(""), None);
    }

    #[test]
    fn extract_meal_time_finds_hhmm_in_parens() {
        assert_eq!(
            extract_meal_time("Breakfast (08:30):"),
            Some("08:30".to_string())
        );
    }

    #[test]
    fn extract_meal_time_returns_none_when_absent() {
        assert_eq!(extract_meal_time("Dinner:"), None);
    }

    #[test]
    fn extract_meal_type_strips_time_and_colon() {
        assert_eq!(extract_meal_type("Breakfast (08:30):"), "Breakfast");
        assert_eq!(extract_meal_type("Dinner:"), "Dinner");
    }

    #[test]
    fn is_meal_header_requires_trailing_colon() {
        assert!(is_meal_header("Breakfast:"));
        assert!(is_meal_header("Breakfast (08:30):"));
        assert!(!is_meal_header("Breakfast"));
        assert!(!is_meal_header(":"));
        assert!(!is_meal_header(""));
    }

    #[test]
    fn plan_serializes_to_expected_json_shape() {
        let plan = Plan {
            start_date: Some("2026-03-04".to_string()),
            end_date: Some("2026-03-04".to_string()),
            days: vec![PlanDay {
                date: Some("2026-03-04".to_string()),
                name: Some("Day 1 (2026-03-04)".to_string()),
                meals: vec![PlanMeal {
                    meal_type: "Breakfast".to_string(),
                    time: Some("08:30".to_string()),
                    recipes: vec![PlanRecipe {
                        name: "pancakes".to_string(),
                        scale: 2.0,
                        scale_unit: None,
                        ingredients: vec![],
                        components: vec![],
                    }],
                }],
                ingredients: vec![],
            }],
            total_meal_count: 1,
            unique_recipe_count: 1,
            all_ingredients: vec![],
            missing_recipes: vec![],
            servings: None,
        };
        let v = serde_json::to_value(&plan).unwrap();
        assert_eq!(v["start_date"], "2026-03-04");
        assert_eq!(v["days"][0]["meals"][0]["meal_type"], "Breakfast");
        assert_eq!(v["days"][0]["meals"][0]["recipes"][0]["scale"], 2.0);
    }

    #[test]
    fn build_plan_two_dated_sections_populate_days_and_range() {
        let menu_src = indoc::indoc! {r#"
            = Day 1 (2026-03-04) =

            = Day 2 (2026-03-05) =
        "#};
        let plan = build_plan_from_source(menu_src, Path::new("."), None).unwrap();
        assert_eq!(plan.days.len(), 2);
        assert_eq!(plan.days[0].date, Some("2026-03-04".to_string()));
        assert_eq!(plan.days[1].date, Some("2026-03-05".to_string()));
        assert_eq!(plan.start_date, Some("2026-03-04".to_string()));
        assert_eq!(plan.end_date, Some("2026-03-05".to_string()));
    }

    #[test]
    fn build_plan_undated_section_yields_day_with_no_date() {
        let menu_src = indoc::indoc! {r#"
            = Plain section =
        "#};
        let plan = build_plan_from_source(menu_src, Path::new("."), None).unwrap();
        assert_eq!(plan.days.len(), 1);
        assert!(plan.days[0].date.is_none());
        assert_eq!(plan.days[0].name.as_deref(), Some("Plain section"));
        assert!(plan.start_date.is_none());
        assert!(plan.end_date.is_none());
    }

    // build_plan_collects_meals_and_recipe_refs_without_expansion was removed in
    // Task 10: now that recipe refs are resolved against disk, the integration
    // test in tests/plan_resolution_test.rs covers the same ground and more.
}
