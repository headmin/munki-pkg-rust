//! Reading application metadata out of a payload.
//!
//! When packaging an app, the version you want is almost always the one the app
//! already declares in its `Info.plist`. Scanning for it removes the step where
//! someone bumps the app but forgets to bump `build-info`, and the package ships
//! claiming the wrong version.
//!
//! The scan deliberately does **not** descend into a bundle it has already
//! matched. A shipping app commonly embeds helper apps — 1Password carries four
//! under `Contents/Frameworks/` — and a naive recursive walk would report five
//! candidates with no way to tell which one is the product.

use crate::errors::invalid_config;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Metadata read from one bundle's `Info.plist`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    /// Path to the bundle itself, e.g. `payload/Applications/Foo.app`.
    pub path: PathBuf,
    /// Bundle path relative to the scanned root, for stable reporting.
    pub relative_path: String,
    /// `CFBundleIdentifier`
    pub bundle_identifier: Option<String>,
    /// `CFBundleName`, falling back to `CFBundleDisplayName`.
    pub name: Option<String>,
    /// `CFBundleShortVersionString` — the marketing version users see.
    pub short_version: Option<String>,
    /// `CFBundleVersion` — the build number.
    pub bundle_version: Option<String>,
    /// `LSMinimumSystemVersion`
    pub minimum_system_version: Option<String>,
}

impl AppInfo {
    /// The version to package with.
    ///
    /// Prefers `CFBundleShortVersionString`; some apps ship only
    /// `CFBundleVersion`, so that is the fallback rather than a failure.
    pub fn version(&self) -> Option<&str> {
        self.short_version
            .as_deref()
            .or(self.bundle_version.as_deref())
    }

    fn read(bundle: &Path, root: &Path) -> Result<Option<Self>> {
        let plist_path = bundle.join("Contents/Info.plist");
        if !plist_path.is_file() {
            // A directory named `.app` without an Info.plist is not a bundle.
            return Ok(None);
        }

        // `plist::Value::from_file` reads both XML and binary property lists,
        // which matters because plenty of shipping apps use the binary form.
        let value = plist::Value::from_file(&plist_path)
            .with_context(|| format!("Failed to parse {}", plist_path.display()))?;

        let Some(dict) = value.as_dictionary() else {
            return Err(invalid_config(format!(
                "{} is not a property list dictionary",
                plist_path.display()
            ))
            .into());
        };

        let get = |key: &str| dict.get(key).and_then(scalar_string);

        Ok(Some(Self {
            relative_path: relative_display(bundle, root),
            path: bundle.to_path_buf(),
            bundle_identifier: get("CFBundleIdentifier"),
            name: get("CFBundleName").or_else(|| get("CFBundleDisplayName")),
            short_version: get("CFBundleShortVersionString"),
            bundle_version: get("CFBundleVersion"),
            minimum_system_version: get("LSMinimumSystemVersion"),
        }))
    }
}

/// Recursively find every top-level `.app` bundle under `root`.
///
/// Results are ordered shallowest first, then alphabetically, so the output is
/// deterministic across machines and filesystems. Bundles nested inside another
/// bundle are not reported — see the module comment.
pub fn scan(root: &Path) -> Result<Vec<AppInfo>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut found = Vec::new();
    let mut walker = WalkDir::new(root).sort_by_file_name().into_iter();

    while let Some(entry) = walker.next() {
        let Ok(entry) = entry else { continue };

        // Symlinks are not followed, so a symlinked bundle is not a candidate.
        if !entry.file_type().is_dir() {
            continue;
        }

        let is_bundle = entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"));

        if !is_bundle {
            continue;
        }

        // Stop here: everything below belongs to this bundle, including any
        // helper apps it embeds.
        walker.skip_current_dir();

        if let Some(info) = AppInfo::read(entry.path(), root)? {
            found.push(info);
        }
    }

    found.sort_by(|a, b| {
        let depth = |p: &str| p.matches('/').count();
        depth(&a.relative_path)
            .cmp(&depth(&b.relative_path))
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });

    Ok(found)
}

/// Find the one app a project packages.
///
/// Errors when there is no app, or when there is more than one and the choice
/// would be a guess. Guessing here would silently stamp a package with the
/// wrong version, which is exactly the failure this feature exists to prevent.
pub fn find_primary(root: &Path) -> Result<AppInfo> {
    let apps = scan(root)?;

    match apps.len() {
        0 => Err(invalid_config(format!(
            "No .app bundle found under {}. App version tokens require an \
             application in the payload; set `version` explicitly, or pass \
             --pkg-version, for a payload without one.",
            root.display()
        ))
        .into()),
        1 => Ok(apps.into_iter().next().unwrap()),
        _ => {
            let candidates = apps
                .iter()
                .map(|a| format!("  {}", a.relative_path))
                .collect::<Vec<_>>()
                .join("\n");
            Err(invalid_config(format!(
                "Found {} .app bundles under {}, so the app version is ambiguous:\n{}\n\
                 Set `version` explicitly, or pass --pkg-version.",
                apps.len(),
                root.display(),
                candidates
            ))
            .into())
        }
    }
}

/// Render a bundle path relative to the scanned root, using forward slashes.
fn relative_display(bundle: &Path, root: &Path) -> String {
    bundle
        .strip_prefix(root)
        .unwrap_or(bundle)
        .to_string_lossy()
        .to_string()
}

/// Coerce a property-list scalar to a string.
///
/// Versions are normally strings, but a plist written by hand can carry
/// `<integer>2</integer>` or `<real>1.5</real>`. Accepting those beats failing
/// on a technicality.
fn scalar_string(value: &plist::Value) -> Option<String> {
    match value {
        plist::Value::String(s) if !s.is_empty() => Some(s.clone()),
        plist::Value::String(_) => None,
        plist::Value::Integer(i) => Some(i.to_string()),
        plist::Value::Real(r) => Some(r.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Write a bundle with an XML Info.plist built from key/value pairs.
    fn make_app(root: &Path, relative: &str, keys: &[(&str, &str)]) {
        let contents = root.join(relative).join("Contents");
        fs::create_dir_all(&contents).unwrap();

        let mut dict = plist::Dictionary::new();
        for (key, value) in keys {
            dict.insert(key.to_string(), plist::Value::String(value.to_string()));
        }
        plist::to_file_xml(contents.join("Info.plist"), &dict).unwrap();
    }

    fn sample_keys() -> Vec<(&'static str, &'static str)> {
        vec![
            ("CFBundleIdentifier", "com.example.Foo"),
            ("CFBundleName", "Foo"),
            ("CFBundleShortVersionString", "3.4.1"),
            ("CFBundleVersion", "3410"),
            ("LSMinimumSystemVersion", "13.0"),
        ]
    }

    #[test]
    fn test_finds_a_single_app() {
        let temp = TempDir::new().unwrap();
        make_app(temp.path(), "Applications/Foo.app", &sample_keys());

        let app = find_primary(temp.path()).unwrap();
        assert_eq!(app.relative_path, "Applications/Foo.app");
        assert_eq!(app.bundle_identifier.as_deref(), Some("com.example.Foo"));
        assert_eq!(app.name.as_deref(), Some("Foo"));
        assert_eq!(app.short_version.as_deref(), Some("3.4.1"));
        assert_eq!(app.bundle_version.as_deref(), Some("3410"));
        assert_eq!(app.minimum_system_version.as_deref(), Some("13.0"));
        assert_eq!(app.version(), Some("3.4.1"));
    }

    /// The reason the walk stops at a matched bundle: shipping apps embed
    /// helpers, and reporting them would make every such payload ambiguous.
    #[test]
    fn test_nested_helper_apps_are_not_candidates() {
        let temp = TempDir::new().unwrap();
        make_app(temp.path(), "Applications/Foo.app", &sample_keys());
        make_app(
            temp.path(),
            "Applications/Foo.app/Contents/Frameworks/Foo Helper.app",
            &[("CFBundleShortVersionString", "9.9.9")],
        );
        make_app(
            temp.path(),
            "Applications/Foo.app/Contents/Library/LoginItems/Launcher.app",
            &[("CFBundleShortVersionString", "0.0.1")],
        );

        let apps = scan(temp.path()).unwrap();
        assert_eq!(apps.len(), 1, "found: {:?}", apps);
        assert_eq!(apps[0].version(), Some("3.4.1"));
    }

    #[test]
    fn test_multiple_top_level_apps_are_ambiguous() {
        let temp = TempDir::new().unwrap();
        make_app(temp.path(), "Applications/Foo.app", &sample_keys());
        make_app(
            temp.path(),
            "Applications/Bar.app",
            &[("CFBundleShortVersionString", "1.0")],
        );

        assert_eq!(scan(temp.path()).unwrap().len(), 2);

        let error = find_primary(temp.path()).unwrap_err().to_string();
        assert!(error.contains("ambiguous"), "got: {}", error);
        assert!(error.contains("Applications/Bar.app"));
        assert!(error.contains("Applications/Foo.app"));
    }

    #[test]
    fn test_no_app_is_a_clear_error() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("usr/local/bin")).unwrap();
        fs::write(temp.path().join("usr/local/bin/tool"), "#!/bin/sh\n").unwrap();

        assert!(scan(temp.path()).unwrap().is_empty());
        let error = find_primary(temp.path()).unwrap_err().to_string();
        assert!(error.contains("No .app bundle found"), "got: {}", error);
    }

    #[test]
    fn test_missing_payload_directory_scans_to_empty() {
        let temp = TempDir::new().unwrap();
        assert!(scan(&temp.path().join("absent")).unwrap().is_empty());
    }

    #[test]
    fn test_directory_named_app_without_info_plist_is_skipped() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("Applications/NotReally.app/Contents")).unwrap();

        assert!(scan(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn test_binary_info_plist_is_read() {
        let temp = TempDir::new().unwrap();
        let contents = temp.path().join("Applications/Bin.app/Contents");
        fs::create_dir_all(&contents).unwrap();

        let mut dict = plist::Dictionary::new();
        dict.insert(
            "CFBundleShortVersionString".to_string(),
            plist::Value::String("7.2".to_string()),
        );
        plist::to_file_binary(contents.join("Info.plist"), &dict).unwrap();

        let app = find_primary(temp.path()).unwrap();
        assert_eq!(app.version(), Some("7.2"));
    }

    #[test]
    fn test_falls_back_to_bundle_version() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Applications/OnlyBuild.app",
            &[("CFBundleVersion", "2024.1")],
        );

        let app = find_primary(temp.path()).unwrap();
        assert_eq!(app.short_version, None);
        assert_eq!(app.version(), Some("2024.1"));
    }

    #[test]
    fn test_app_without_any_version_has_none() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Applications/Bare.app",
            &[("CFBundleIdentifier", "com.example.bare")],
        );

        let app = find_primary(temp.path()).unwrap();
        assert_eq!(app.version(), None);
    }

    /// An empty string is not a usable version; treat it as absent so the
    /// fallback to CFBundleVersion still applies.
    #[test]
    fn test_empty_version_string_is_treated_as_absent() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Applications/Empty.app",
            &[
                ("CFBundleShortVersionString", ""),
                ("CFBundleVersion", "12"),
            ],
        );

        let app = find_primary(temp.path()).unwrap();
        assert_eq!(app.version(), Some("12"));
    }

    #[test]
    fn test_non_string_version_is_coerced() {
        let temp = TempDir::new().unwrap();
        let contents = temp.path().join("Applications/Numeric.app/Contents");
        fs::create_dir_all(&contents).unwrap();

        let mut dict = plist::Dictionary::new();
        dict.insert(
            "CFBundleShortVersionString".to_string(),
            plist::Value::Integer(3.into()),
        );
        plist::to_file_xml(contents.join("Info.plist"), &dict).unwrap();

        assert_eq!(find_primary(temp.path()).unwrap().version(), Some("3"));
    }

    #[test]
    fn test_display_name_is_used_when_bundle_name_is_absent() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Applications/Display.app",
            &[
                ("CFBundleDisplayName", "Pretty Name"),
                ("CFBundleShortVersionString", "1.0"),
            ],
        );

        assert_eq!(
            find_primary(temp.path()).unwrap().name.as_deref(),
            Some("Pretty Name")
        );
    }

    #[test]
    fn test_uppercase_extension_is_matched() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Applications/Shouty.APP",
            &[("CFBundleShortVersionString", "5.0")],
        );

        assert_eq!(find_primary(temp.path()).unwrap().version(), Some("5.0"));
    }

    #[test]
    fn test_results_are_ordered_shallowest_first() {
        let temp = TempDir::new().unwrap();
        make_app(
            temp.path(),
            "Library/Deep/Nested/Deep.app",
            &[("CFBundleVersion", "1")],
        );
        make_app(temp.path(), "Top.app", &[("CFBundleVersion", "2")]);

        let apps = scan(temp.path()).unwrap();
        assert_eq!(apps[0].relative_path, "Top.app");
        assert_eq!(apps[1].relative_path, "Library/Deep/Nested/Deep.app");
    }

    #[test]
    fn test_malformed_plist_is_an_error_not_a_silent_skip() {
        let temp = TempDir::new().unwrap();
        let contents = temp.path().join("Applications/Broken.app/Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(contents.join("Info.plist"), "this is not a plist").unwrap();

        assert!(scan(temp.path()).is_err());
    }
}
