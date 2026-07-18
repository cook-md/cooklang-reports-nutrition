//! Client profile loaded from YAML. `Target` uses tagged ranges:
//! macros pick the field they need; missing fields silently skip.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, serde::Serialize)]
pub struct ClientProfile {
    pub name: String,
    #[serde(default)]
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, serde::Serialize)]
pub struct Target {
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub tol_pct: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("failed to read client profile {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse client profile {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

impl ClientProfile {
    pub fn from_yaml_str(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }

    pub fn from_path(path: &Path) -> Result<Self, ClientError> {
        let body = std::fs::read_to_string(path).map_err(|e| ClientError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Self::from_yaml_str(&body).map_err(|e| ClientError::Parse {
            path: path.display().to_string(),
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_profile() {
        let yaml = r#"
name: Sara Mendez
exclusions: [peanut, shellfish]
targets:
  protein_g: { min: 75 }
  sodium_mg: { max: 1500 }
  sat_fat_g: { max: 20, tol_pct: 10 }
  energy_kcal: { min: 1800, max: 2200 }
"#;
        let p = ClientProfile::from_yaml_str(yaml).unwrap();
        assert_eq!(p.name, "Sara Mendez");
        assert_eq!(p.exclusions, vec!["peanut", "shellfish"]);
        let protein = &p.targets["protein_g"];
        assert_eq!(protein.min, Some(75.0));
        assert_eq!(protein.max, None);
        let sat = &p.targets["sat_fat_g"];
        assert_eq!(sat.max, Some(20.0));
        assert_eq!(sat.tol_pct, Some(10.0));
        let energy = &p.targets["energy_kcal"];
        assert_eq!(energy.min, Some(1800.0));
        assert_eq!(energy.max, Some(2200.0));
    }

    #[test]
    fn parses_minimal_profile() {
        let yaml = "name: Empty\n";
        let p = ClientProfile::from_yaml_str(yaml).unwrap();
        assert_eq!(p.name, "Empty");
        assert!(p.exclusions.is_empty());
        assert!(p.targets.is_empty());
    }

    #[test]
    fn target_with_only_min() {
        let yaml = "name: X\ntargets:\n  fiber_g: { min: 25 }\n";
        let p = ClientProfile::from_yaml_str(yaml).unwrap();
        let t = &p.targets["fiber_g"];
        assert_eq!(t.min, Some(25.0));
        assert_eq!(t.max, None);
        assert_eq!(t.tol_pct, None);
    }

    #[test]
    fn parse_error_on_malformed_yaml() {
        let yaml = "name: : bad\n";
        assert!(ClientProfile::from_yaml_str(yaml).is_err());
    }

    #[test]
    fn from_path_reports_missing_file() {
        let err = ClientProfile::from_path(Path::new("/nonexistent/path.yaml")).unwrap_err();
        assert!(matches!(err, ClientError::Io { .. }));
    }
}
