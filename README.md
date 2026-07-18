# cooklang-reports-nutrition

[![CI](https://github.com/cook-md/cooklang-reports-nutrition/actions/workflows/ci.yml/badge.svg)](https://github.com/cook-md/cooklang-reports-nutrition/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cooklang-reports-nutrition.svg)](https://crates.io/crates/cooklang-reports-nutrition)
[![crates.io](https://img.shields.io/crates/v/cookmd-nutrition-client.svg)](https://crates.io/crates/cookmd-nutrition-client)

Nutrition facts, % daily values, allergens, and pass/fail dietary checks —
right inside your [cooklang-reports](https://github.com/cooklang/cooklang-reports)
jinja templates. This extension registers functions like `nutrition_for()`,
`total_calories()`, `dv_percent()`, and `record_check()` that resolve real
[Cooklang](https://cooklang.org) ingredients against the
[cook.md](https://cook.md) nutrition service, so a recipe report can state
what's in the dish and whether it meets your targets.

Two crates:

- **`cooklang-reports-nutrition`** — the cooklang-reports extension (jinja functions, dietary checks, `.menu` plan support).
- **`cookmd-nutrition-client`** — a small blocking HTTP client for the cook.md nutrition service, usable on its own.

## Install

```sh
cargo add cooklang-reports-nutrition
```

## Quickstart

```rust
use std::sync::Arc;

use cooklang_reports::{Config, render_template_with_config};
use cooklang_reports_nutrition::NutritionExtension;
use cookmd_nutrition_client::Client;

fn main() {
    let base_url = std::env::var("NUTRITION_API_URL")
        .unwrap_or_else(|_| "https://nutrition.cook.md".into());
    let client = Client::new(base_url)
        .with_auth_token(std::env::var("COOKMD_TOKEN").expect("set COOKMD_TOKEN"));
    let ext = NutritionExtension::new(Arc::new(client));

    let config = Config::builder()
        .base_path(std::path::Path::new("."))
        .build()
        .with_extension(ext);

    let recipe = std::fs::read_to_string("dinner.cook").unwrap();
    let template = std::fs::read_to_string("report.md.jinja").unwrap();
    println!("{}", render_template_with_config(&recipe, &template, &config).unwrap());
}
```

And a `report.md.jinja` to go with it:

```jinja
Calories: {{ total_calories(ingredients) }} kcal
{% for ing in ingredients %}{% set n = nutrition_for(ing) -%}
- {{ ing.name }}: {{ n.protein_g }} g protein
  {{- " — PASS" if compare(n.protein_g, 20, "gte") else " — FAIL" }}
{% endfor %}
```

## Demo

The repo ships a demo binary with recipe, menu-plan, and dietary-check
fixtures. Point `NUTRITION_API_URL` at a nutrition service and run:

```sh
# Single recipe with pass/fail protein checks (exit code 1 if any check fails):
cargo run -p cook-nutrition-demo -- \
    examples-bin/cook-nutrition-demo/fixtures/nutrition-check-pass.md.jinja \
    examples-bin/cook-nutrition-demo/fixtures/salmon-dinner.cook

# A week's .menu plan aggregated across recipes:
cargo run -p cook-nutrition-demo -- \
    --base-path examples-bin/cook-nutrition-demo/fixtures \
    examples-bin/cook-nutrition-demo/fixtures/menu-pass.md.jinja \
    examples-bin/cook-nutrition-demo/fixtures/week.menu
```

## Auth & service

The extension talks to the cook.md nutrition service. To use the hosted
service you need a [cook.md](https://cook.md) account with Cook Pro, or an
organization API key; pass the token via `Client::with_auth_token(...)`.

- `NUTRITION_API_URL` selects the service (the demo defaults to
  `http://127.0.0.1:8080`; the hosted service is `https://nutrition.cook.md`).
- The full template function reference (all functions, arguments, and return
  shapes) lives at **<https://nutrition.cook.md/docs/guides>**.
- Prefer an agent tool instead of jinja? The same stack is packaged as an MCP
  server: `npx -y @cookmd/nutrition-mcp`.

## License

MIT — see [LICENSE](LICENSE).
