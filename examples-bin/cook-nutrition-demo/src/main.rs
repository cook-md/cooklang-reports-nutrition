use std::process::ExitCode;
use std::sync::Arc;

use cooklang_reports::{Config, render_template_with_config};
use cooklang_reports_nutrition::NutritionExtension;
use cookmd_nutrition_client::Client;

struct Args {
    template: String,
    recipe: String,
    base_path: Option<String>,
    client: Option<String>,
}

fn parse_args() -> Option<Args> {
    let raw: Vec<String> = std::env::args().collect();
    let mut template = None;
    let mut recipe = None;
    let mut base_path = None;
    let mut client = None;
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--base-path" => {
                i += 1;
                match raw.get(i) {
                    Some(v) => base_path = Some(v.clone()),
                    None => {
                        eprintln!("--base-path requires an argument");
                        return None;
                    }
                }
            }
            "--client" => {
                i += 1;
                match raw.get(i) {
                    Some(v) => client = Some(v.clone()),
                    None => {
                        eprintln!("--client requires an argument");
                        return None;
                    }
                }
            }
            arg if template.is_none() => template = Some(arg.to_string()),
            arg if recipe.is_none() => recipe = Some(arg.to_string()),
            _ => return None,
        }
        i += 1;
    }
    Some(Args {
        template: template?,
        recipe: recipe?,
        base_path,
        client,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Some(a) => a,
        None => {
            eprintln!(
                "usage: cook-nutrition-demo [--base-path <dir>] [--client <profile.yaml>] <template.jinja> <input.{{cook,menu}}>"
            );
            return ExitCode::from(2);
        }
    };
    let template = match std::fs::read_to_string(&args.template) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read template {}: {e}", args.template);
            return ExitCode::from(2);
        }
    };
    let recipe = match std::fs::read_to_string(&args.recipe) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read recipe {}: {e}", args.recipe);
            return ExitCode::from(2);
        }
    };

    let base_url =
        std::env::var("NUTRITION_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let client = Arc::new(Client::new(base_url));
    let ext = NutritionExtension::new(client);

    let ext = if let Some(ref path) = args.client {
        match ext.with_client_path(std::path::Path::new(path)) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("client profile error: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        ext
    };

    let client_ctx = ext.client_context();
    let tracker = ext.checks();

    let base_path_buf;
    let base_path: &std::path::Path = if let Some(ref bp) = args.base_path {
        base_path_buf = match std::path::Path::new(bp).canonicalize() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("failed to resolve --base-path {bp}: {e}");
                return ExitCode::from(2);
            }
        };
        &base_path_buf
    } else {
        let input_path = match std::path::Path::new(&args.recipe).canonicalize() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("failed to resolve path {}: {e}", args.recipe);
                return ExitCode::from(2);
            }
        };
        base_path_buf = input_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        &base_path_buf
    };
    let input_kind = std::path::Path::new(&args.recipe)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let mut config = Config::builder()
        .base_path(base_path)
        .build()
        .with_extension(ext);

    if let Some(ctx) = client_ctx {
        config = config.with_context("client", ctx);
    }

    if input_kind == "menu" {
        let plan = match cooklang_reports_nutrition::plan::build_plan_from_source(
            &recipe,
            base_path,
            Some(std::path::Path::new(&args.recipe)),
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("plan error: {e}");
                return ExitCode::from(2);
            }
        };
        let plan_json = match serde_json::to_value(&plan) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("failed to serialize plan: {e}");
                return ExitCode::from(2);
            }
        };
        config = config.with_context("plan", plan_json);
    }

    // `.menu` files aren't valid `.cook` recipes — render against an empty
    // recipe so cooklang-reports' parse step doesn't choke on menu syntax.
    // Templates consume the menu through `plan.*` instead.
    let render_source: &str = if input_kind == "menu" { "" } else { &recipe };

    let rendered = match render_template_with_config(render_source, &template, &config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("render error: {e}");
            return ExitCode::from(2);
        }
    };
    print!("{rendered}");

    let failed = tracker.failed_count();
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
