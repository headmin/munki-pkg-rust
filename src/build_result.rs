//! Machine-readable summary of a completed build.

use crate::hash::sha256_file;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What one build produced.
///
/// The booleans report what *happened*, not what build-info requested: a
/// project configured for notarization that was built with `--skip-notarization`
/// reports `notarized: false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildResult {
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub pkg_path: String,
    pub sha256: String,
    pub signed: bool,
    pub notarized: bool,
    pub stapled: bool,
}

impl BuildResult {
    /// Assemble a result for a finished package, hashing it in the process.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        identifier: impl Into<String>,
        pkg_path: &Path,
        signed: bool,
        notarized: bool,
        stapled: bool,
    ) -> Result<Self> {
        Ok(Self {
            name: name.into(),
            version: version.into(),
            identifier: identifier.into(),
            pkg_path: pkg_path.display().to_string(),
            sha256: sha256_file(pkg_path)?,
            signed,
            notarized,
            stapled,
        })
    }

    /// Pretty-printed JSON manifest for `--output-format json`.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample() -> (TempDir, BuildResult) {
        let temp = TempDir::new().unwrap();
        let pkg = temp.path().join("mypackage-1.0.pkg");
        fs::write(&pkg, b"abc").unwrap();

        let result = BuildResult::new(
            "mypackage-1.0.pkg",
            "1.0",
            "com.example.mypackage",
            &pkg,
            true,
            false,
            false,
        )
        .unwrap();

        (temp, result)
    }

    #[test]
    fn test_hashes_the_package() {
        let (_temp, result) = sample();
        assert_eq!(
            result.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_booleans_report_what_happened() {
        let (_temp, result) = sample();
        assert!(result.signed);
        assert!(!result.notarized);
        assert!(!result.stapled);
    }

    #[test]
    fn test_json_uses_the_documented_keys() {
        let (_temp, result) = sample();
        let json: serde_json::Value = serde_json::from_str(&result.to_json().unwrap()).unwrap();

        for key in [
            "name",
            "version",
            "identifier",
            "pkg_path",
            "sha256",
            "signed",
            "notarized",
            "stapled",
        ] {
            assert!(json.get(key).is_some(), "manifest is missing {}", key);
        }
        assert_eq!(json["identifier"], "com.example.mypackage");
        assert_eq!(json["signed"], true);
    }

    #[test]
    fn test_json_round_trips() {
        let (_temp, result) = sample();
        let parsed: BuildResult = serde_json::from_str(&result.to_json().unwrap()).unwrap();
        assert_eq!(parsed, result);
    }
}
