//! Post-build verification.
//!
//! Asserts the finished package is actually the thing build-info described: a
//! stale artifact left in `build/`, a mismatched version, or a signature that
//! silently failed will not pass.

use crate::errors::build_failed;
use crate::external::{PKGUTIL, SPCTL, pkgutil_expand, run_command};
use anyhow::{Context, Result};
use regex_lite::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Verify a built package against what build-info declared.
///
/// `signed` and `notarized` say whether those steps actually ran; each enables
/// the corresponding external check.
pub fn verify_package(
    pkg_path: &Path,
    expected_identifier: &str,
    expected_version: &str,
    signed: bool,
    notarized: bool,
    quiet: bool,
) -> Result<()> {
    verify_metadata(pkg_path, expected_identifier, expected_version, quiet)?;

    if signed {
        let mut cmd = Command::new(PKGUTIL);
        cmd.arg("--check-signature").arg(pkg_path);
        let output = run_command(&mut cmd)?;
        if !output.status.success() {
            return Err(build_failed(format!(
                "Verification failed: package is not validly signed. {}",
                diagnostics(&output)
            ))
            .into());
        }
        if !quiet {
            println!("Verified package signature");
        }
    }

    if notarized {
        let mut cmd = Command::new(SPCTL);
        cmd.args(["-a", "-vvv", "-t", "install"]).arg(pkg_path);
        let output = run_command(&mut cmd)?;
        if !output.status.success() {
            return Err(build_failed(format!(
                "Verification failed: package does not pass Gatekeeper assessment. {}",
                diagnostics(&output)
            ))
            .into());
        }
        if !quiet {
            println!("Verified Gatekeeper assessment");
        }
    }

    Ok(())
}

/// Confirm the package embeds the identifier and version build-info declared.
///
/// Component packages carry this in `PackageInfo`; distribution packages carry
/// it in the expanded `Distribution` document.
fn verify_metadata(
    pkg_path: &Path,
    expected_identifier: &str,
    expected_version: &str,
    quiet: bool,
) -> Result<()> {
    let temp = TempDir::new().context("Failed to create temporary directory")?;
    let expanded = temp.path().join("expanded");

    pkgutil_expand(pkg_path, &expanded).map_err(|error| {
        build_failed(format!(
            "Verification failed: could not expand {} to inspect its metadata. {:#}",
            pkg_path.display(),
            error
        ))
    })?;

    let distribution = expanded.join("Distribution");
    let mismatch = if distribution.is_file() {
        let xml = read_metadata(&distribution, pkg_path, "Distribution")?;
        distribution_metadata_mismatch(expected_identifier, expected_version, &xml)
    } else {
        let package_info = expanded.join("PackageInfo");
        let xml = read_metadata(&package_info, pkg_path, "PackageInfo")?;
        metadata_mismatch(expected_identifier, expected_version, &xml)
    };

    if let Some(mismatch) = mismatch {
        return Err(build_failed(format!("Verification failed: {}", mismatch)).into());
    }

    if !quiet {
        println!("Verified package identifier and version");
    }

    Ok(())
}

fn read_metadata(path: &Path, pkg_path: &Path, kind: &str) -> Result<String> {
    fs::read_to_string(path).map_err(|error| {
        build_failed(format!(
            "Verification failed: expanded {} is missing readable {} metadata at {}. {}",
            pkg_path.display(),
            kind,
            path.display(),
            error
        ))
        .into()
    })
}

/// Extract an attribute from the first element with the given tag name.
fn attribute(xml: &str, element: &str, attribute: &str) -> Option<String> {
    let element_pattern = format!(r"<{}\b[^>]*>", regex_lite::escape(element));
    let element_re = Regex::new(&element_pattern).ok()?;
    let tag = element_re.find(xml)?.as_str();

    let attribute_pattern = format!(r#"\b{}\s*=\s*"([^"]*)""#, regex_lite::escape(attribute));
    let attribute_re = Regex::new(&attribute_pattern).ok()?;

    attribute_re
        .captures(tag)
        .map(|caps| unescape_xml(&caps[1]))
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Compare a `PackageInfo` document against build-info. Returns a human-readable
/// mismatch, or `None` when they agree.
///
/// Pure, so it is unit-tested without building a package.
pub fn metadata_mismatch(
    expected_identifier: &str,
    expected_version: &str,
    package_info_xml: &str,
) -> Option<String> {
    if !package_info_xml.contains("<pkg-info") {
        return Some("package PackageInfo is missing a pkg-info element.".to_string());
    }
    compare(
        expected_identifier,
        expected_version,
        attribute(package_info_xml, "pkg-info", "identifier"),
        attribute(package_info_xml, "pkg-info", "version"),
        "package PackageInfo is missing an identifier.",
        "package PackageInfo is missing a version.",
        "package identifier",
        "package version",
    )
}

/// Compare the `product` element `productbuild` writes into a distribution
/// package against build-info.
pub fn distribution_metadata_mismatch(
    expected_identifier: &str,
    expected_version: &str,
    distribution_xml: &str,
) -> Option<String> {
    if !distribution_xml.contains("<product") {
        return Some("package Distribution metadata is missing a product element.".to_string());
    }
    compare(
        expected_identifier,
        expected_version,
        attribute(distribution_xml, "product", "id"),
        attribute(distribution_xml, "product", "version"),
        "package Distribution product metadata is missing an identifier.",
        "package Distribution product metadata is missing a version.",
        "package distribution identifier",
        "package distribution version",
    )
}

#[allow(clippy::too_many_arguments)]
fn compare(
    expected_identifier: &str,
    expected_version: &str,
    actual_identifier: Option<String>,
    actual_version: Option<String>,
    missing_identifier: &str,
    missing_version: &str,
    identifier_label: &str,
    version_label: &str,
) -> Option<String> {
    match actual_identifier {
        None => return Some(missing_identifier.to_string()),
        Some(identifier) if identifier.is_empty() => return Some(missing_identifier.to_string()),
        Some(identifier) if identifier != expected_identifier => {
            return Some(format!(
                "{} is \"{}\" but build-info declares \"{}\".",
                identifier_label, identifier, expected_identifier
            ));
        }
        Some(_) => {}
    }

    match actual_version {
        None => Some(missing_version.to_string()),
        Some(version) if version.is_empty() => Some(missing_version.to_string()),
        Some(version) if version != expected_version => Some(format!(
            "{} is \"{}\" but build-info declares \"{}\".",
            version_label, version, expected_version
        )),
        Some(_) => None,
    }
}

fn diagnostics(output: &std::process::Output) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        "(no output)".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_info(identifier: &str, version: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<pkg-info overwrite-permissions="true" relocatable="false" identifier="{}" version="{}" install-location="/" auth="root">
    <payload numberOfFiles="3" installKBytes="12"/>
</pkg-info>"#,
            identifier, version
        )
    }

    fn distribution(id: &str, version: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="1">
    <product id="{}" version="{}"/>
    <pkg-ref id="com.example.app"/>
</installer-gui-script>"#,
            id, version
        )
    }

    #[test]
    fn test_matching_package_info_has_no_mismatch() {
        let xml = package_info("com.example.app", "1.0");
        assert_eq!(metadata_mismatch("com.example.app", "1.0", &xml), None);
    }

    #[test]
    fn test_identifier_mismatch_is_reported() {
        let xml = package_info("com.example.other", "1.0");
        let mismatch = metadata_mismatch("com.example.app", "1.0", &xml).unwrap();
        assert!(mismatch.contains("com.example.other"));
        assert!(mismatch.contains("com.example.app"));
    }

    #[test]
    fn test_version_mismatch_is_reported() {
        let xml = package_info("com.example.app", "0.9");
        let mismatch = metadata_mismatch("com.example.app", "1.0", &xml).unwrap();
        assert!(mismatch.contains("\"0.9\""));
        assert!(mismatch.contains("\"1.0\""));
    }

    #[test]
    fn test_missing_pkg_info_element() {
        let mismatch = metadata_mismatch("com.example.app", "1.0", "<other/>").unwrap();
        assert!(mismatch.contains("missing a pkg-info element"));
    }

    #[test]
    fn test_missing_identifier_attribute() {
        let xml = r#"<pkg-info version="1.0"/>"#;
        let mismatch = metadata_mismatch("com.example.app", "1.0", xml).unwrap();
        assert!(mismatch.contains("missing an identifier"));
    }

    #[test]
    fn test_missing_version_attribute() {
        let xml = r#"<pkg-info identifier="com.example.app"/>"#;
        let mismatch = metadata_mismatch("com.example.app", "1.0", xml).unwrap();
        assert!(mismatch.contains("missing a version"));
    }

    #[test]
    fn test_empty_identifier_attribute_is_missing() {
        let xml = r#"<pkg-info identifier="" version="1.0"/>"#;
        let mismatch = metadata_mismatch("com.example.app", "1.0", xml).unwrap();
        assert!(mismatch.contains("missing an identifier"));
    }

    #[test]
    fn test_matching_distribution_has_no_mismatch() {
        let xml = distribution("com.example.app", "1.0");
        assert_eq!(
            distribution_metadata_mismatch("com.example.app", "1.0", &xml),
            None
        );
    }

    #[test]
    fn test_distribution_identifier_mismatch() {
        let xml = distribution("com.example.stale", "1.0");
        let mismatch = distribution_metadata_mismatch("com.example.app", "1.0", &xml).unwrap();
        assert!(mismatch.contains("distribution identifier"));
        assert!(mismatch.contains("com.example.stale"));
    }

    #[test]
    fn test_distribution_version_mismatch() {
        let xml = distribution("com.example.app", "2.0");
        let mismatch = distribution_metadata_mismatch("com.example.app", "1.0", &xml).unwrap();
        assert!(mismatch.contains("distribution version"));
    }

    #[test]
    fn test_missing_product_element() {
        let xml = "<installer-gui-script minSpecVersion=\"1\"/>";
        let mismatch = distribution_metadata_mismatch("com.example.app", "1.0", xml).unwrap();
        assert!(mismatch.contains("missing a product element"));
    }

    /// `pkg-ref` also carries an `id`; the product element must be the one read.
    #[test]
    fn test_product_element_is_preferred_over_pkg_ref() {
        let xml = r#"<installer-gui-script>
    <pkg-ref id="com.example.wrong"/>
    <product id="com.example.app" version="1.0"/>
</installer-gui-script>"#;
        assert_eq!(
            distribution_metadata_mismatch("com.example.app", "1.0", xml),
            None
        );
    }

    #[test]
    fn test_attributes_with_single_spacing_variants() {
        let xml = r#"<pkg-info identifier = "com.example.app"  version="1.0" />"#;
        assert_eq!(metadata_mismatch("com.example.app", "1.0", xml), None);
    }

    #[test]
    fn test_xml_entities_are_unescaped() {
        let xml = r#"<pkg-info identifier="com.example.a&amp;b" version="1.0"/>"#;
        assert_eq!(metadata_mismatch("com.example.a&b", "1.0", xml), None);
    }

    #[test]
    fn test_version_with_dynamic_token_result() {
        let xml = package_info("com.example.app", "2026.07.18.1405");
        assert_eq!(
            metadata_mismatch("com.example.app", "2026.07.18.1405", &xml),
            None
        );
    }
}
