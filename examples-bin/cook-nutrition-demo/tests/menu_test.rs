use std::process::Command;

fn binary_path() -> String {
    env!("CARGO_BIN_EXE_cook-nutrition-demo").to_string()
}

fn fixtures(rel: &str) -> String {
    format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

#[test]
fn menu_pass_template_exits_zero() {
    // No NUTRITION_API_URL env needed: menu-pass.md.jinja only reads `plan.*`,
    // which is built locally without hitting the nutrition service.
    let output = Command::new(binary_path())
        .args([
            "--base-path",
            &fixtures(""),
            &fixtures("menu-pass.md.jinja"),
            &fixtures("week.menu"),
        ])
        .output()
        .expect("run demo");
    assert!(
        output.status.success(),
        "demo exited {}: stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FAILED: 0"));
    assert!(stdout.contains("Days: 2"));
    assert!(stdout.contains("Meals: 3"));
}

#[test]
fn menu_missing_recipe_ref_is_surfaced_not_fatal() {
    // A dangling recipe ref must not abort the render: the rest of the plan
    // still resolves, and the failure is exposed as `plan.missing_recipes`
    // for the template to display.
    let tmp = tempfile::tempdir().expect("create tempdir");
    let bogus_menu = tmp.path().join("missing-ref.menu");
    std::fs::write(
        &bogus_menu,
        "= Day 1 =\n\nBreakfast:\n\n@./does-not-exist{1}\n",
    )
    .expect("write fixture");
    let template = tmp.path().join("missing.md.jinja");
    std::fs::write(
        &template,
        "missing={{ plan.missing_recipes | length }} first={{ plan.missing_recipes[0].name }} err={{ plan.missing_recipes[0].error }}",
    )
    .expect("write template");

    let output = Command::new(binary_path())
        .args([
            "--base-path",
            &fixtures(""),
            template.to_str().expect("utf-8 path"),
            bogus_menu.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("run demo");

    assert!(
        output.status.success(),
        "demo exited {}: stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("missing=1"), "stdout: {stdout}");
    assert!(stdout.contains("first=does-not-exist"), "stdout: {stdout}");
    assert!(stdout.contains("recipe not found"), "stdout: {stdout}");
}
