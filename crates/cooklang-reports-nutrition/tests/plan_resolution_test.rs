use std::path::PathBuf;

use cooklang_reports_nutrition::plan::build_plan_from_source;

fn fixtures_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p
}

#[test]
fn build_plan_resolves_recipe_ref_to_ingredients_scaled() {
    let menu = "= Day 1 =\n\nBreakfast:\n\n@./pancakes{2}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let recipe = &plan.days[0].meals[0].recipes[0];
    assert_eq!(recipe.name, "pancakes");
    assert_eq!(recipe.scale, 2.0);
    // Two pancakes' worth, so flour goes 100g -> 200g.
    let flour = recipe
        .ingredients
        .iter()
        .find(|i| i.name == "flour")
        .expect("flour in expanded pancakes");
    let q = flour.quantity.as_ref().expect("flour has quantity");
    assert_eq!(q.value, 200.0);
    assert_eq!(q.unit, "g");
    // plan.all_ingredients aggregates across all meals/recipes.
    let kinds: Vec<&str> = plan
        .all_ingredients
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert!(kinds.contains(&"flour"));
    assert!(kinds.contains(&"milk"));
    assert!(kinds.contains(&"eggs"));
}

#[test]
fn build_plan_resolves_subdirectory_recipe_ref() {
    // Real recipe libraries nest recipes (`@./Salads/Caprese{3%servings}`);
    // the reference's directory components must survive into path resolution.
    let menu = "= Day 1 =\n\nDinner:\n\n@./salads/caprese{3%servings}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let recipe = &plan.days[0].meals[0].recipes[0];
    assert_eq!(recipe.name, "caprese");
    assert_eq!(recipe.scale, 3.0);
    let mozzarella = recipe
        .ingredients
        .iter()
        .find(|i| i.name == "mozzarella")
        .expect("mozzarella in expanded caprese");
    let q = mozzarella.quantity.as_ref().expect("mozzarella quantity");
    assert_eq!(q.value, 375.0);
    assert_eq!(q.unit, "g");
    assert!(plan.all_ingredients.iter().any(|i| i.name == "olive oil"));
}

#[test]
fn build_plan_detects_meal_headers_with_inline_bullet_refs() {
    // `Breakfast:\n- @./ref{}` parses to ONE step whose text collapses the
    // newline ("Breakfast: - "); header detection must still split meals.
    let menu = "== Day 1 (2026-03-04) ==\n\nBreakfast:\n- @./pancakes{1}\n\nLunch (12:30):\n- @./salads/caprese{1}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let meals = &plan.days[0].meals;
    assert_eq!(meals.len(), 2, "each header opens its own meal");
    assert_eq!(meals[0].meal_type, "Breakfast");
    assert_eq!(meals[0].recipes[0].name, "pancakes");
    assert_eq!(meals[1].meal_type, "Lunch");
    assert_eq!(meals[1].time.as_deref(), Some("12:30"));
    assert_eq!(meals[1].recipes[0].name, "caprese");
}

#[test]
fn build_plan_includes_loose_menu_ingredients() {
    // Menus mix recipe refs with standalone items (a Snacks section); loose
    // ingredients must land in day + plan totals or the report undercounts.
    let menu = "= Day 1 =\n\nBreakfast:\n\n@./pancakes{1}\n\n= Snacks =\n\n@apples{5}\n@kefir{1%l}\n@cucumber{}(sliced)\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let names: Vec<&str> = plan
        .all_ingredients
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert!(names.contains(&"flour"), "recipe expansion still works");
    assert!(names.contains(&"apples"));
    assert!(names.contains(&"kefir"));
    let kefir = plan
        .all_ingredients
        .iter()
        .find(|i| i.name == "kefir")
        .unwrap();
    let q = kefir.quantity.as_ref().expect("kefir has quantity");
    assert_eq!(q.value, 1.0);
    assert_eq!(q.unit, "l");
    let cucumber = plan
        .all_ingredients
        .iter()
        .find(|i| i.name == "cucumber")
        .unwrap();
    assert_eq!(cucumber.note.as_deref(), Some("sliced"));
    // Loose items are day-scoped too (the Snacks section is its own day entry).
    let snacks_day = plan
        .days
        .iter()
        .find(|d| !d.ingredients.is_empty() && d.ingredients.iter().any(|i| i.name == "apples"))
        .expect("snacks day");
    assert!(snacks_day.ingredients.iter().any(|i| i.name == "kefir"));
}

#[test]
fn build_plan_records_missing_ref_and_keeps_the_rest() {
    // A dangling reference must not kill the plan — the resolvable recipe
    // still expands and the failure is surfaced in `missing_recipes`.
    let menu = "= Day 1 =\n\nBreakfast:\n\n@./missing{1}\n\nLunch:\n\n@./pancakes{1}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    assert_eq!(plan.missing_recipes.len(), 1);
    assert_eq!(plan.missing_recipes[0].name, "missing");
    assert!(plan.missing_recipes[0].error.contains("recipe not found"));
    // The missing recipe contributes no ingredients; pancakes still does.
    assert!(plan.all_ingredients.iter().any(|i| i.name == "flour"));
    assert!(
        plan.days[0].meals[0].recipes[0].ingredients.is_empty(),
        "missing recipe must not gain ingredients"
    );
}

#[test]
fn build_plan_records_recursive_recipe_ref_as_missing() {
    let menu = "= Day 1 =\n\nBreakfast:\n\n@./loop_a{1}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    assert_eq!(plan.missing_recipes.len(), 1);
    assert_eq!(plan.missing_recipes[0].name, "loop_a");
    assert!(plan.missing_recipes[0].error.contains("recursive"));
}

/// Mass of an expanded ingredient in grams (the converter may refit g → kg).
fn grams(recipe: &cooklang_reports_nutrition::plan::PlanRecipe, name: &str) -> f64 {
    let q = recipe
        .ingredients
        .iter()
        .find(|i| i.name == name)
        .and_then(|i| i.quantity.as_ref())
        .unwrap_or_else(|| panic!("{name} with quantity"));
    match q.unit.as_str() {
        "kg" => q.value * 1000.0,
        "g" => q.value,
        other => panic!("unexpected unit {other} for {name}"),
    }
}

#[test]
fn build_plan_servings_ref_scales_to_target_servings() {
    // `{3%servings}` asks for 3 servings of a 6-serving recipe: factor 0.5,
    // not a raw ×3 multiplier.
    let menu = "= Day 1 =\n\nDinner:\n\n@./stew{3%servings}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let recipe = &plan.days[0].meals[0].recipes[0];
    assert_eq!(recipe.scale, 0.5);
    let beef = recipe
        .ingredients
        .iter()
        .find(|i| i.name == "beef")
        .expect("beef in expanded stew");
    let q = beef.quantity.as_ref().expect("beef quantity");
    assert_eq!(q.value, 600.0);
    assert_eq!(q.unit, "g");
}

#[test]
fn build_plan_bare_number_ref_stays_a_multiplier() {
    // `{2}` (no unit) is a plain scaling factor even when the recipe declares
    // servings.
    let menu = "= Day 1 =\n\nDinner:\n\n@./stew{2}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let recipe = &plan.days[0].meals[0].recipes[0];
    assert_eq!(recipe.scale, 2.0);
    assert_eq!(grams(recipe, "beef"), 2400.0);
}

#[test]
fn build_plan_nested_servings_ref_scales_to_target_servings() {
    // stew_dinner (2 servings) pulls in `@./stew{3%servings}`. Asking for 4
    // servings of stew_dinner doubles it → 6 servings of stew → factor 1.0.
    let menu = "= Day 1 =\n\nDinner:\n\n@./stew_dinner{4%servings}\n";
    let plan = build_plan_from_source(menu, &fixtures_dir(), None).unwrap();
    let recipe = &plan.days[0].meals[0].recipes[0];
    assert_eq!(recipe.scale, 2.0);
    assert_eq!(grams(recipe, "beef"), 1200.0);
    let bread = recipe
        .ingredients
        .iter()
        .find(|i| i.name == "bread")
        .unwrap();
    assert_eq!(bread.quantity.as_ref().unwrap().value, 4.0);
}
