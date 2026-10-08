//! Package import functionality
//!
//! Import existing .pkg files into munki-pkg project format.
//!
//! Original algorithm by Greg Neagle in munki-pkg.

use crate::config::{BuildInfo, OutputFormat};
use crate::external::{ditto_extract, lsbom_extract, pkgutil_expand};
use crate::sync::apply_bom_to_payload;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use walkdir::WalkDir;

/// Default .gitignore content for imported projects
const DEFAULT_GITIGNORE: &str = r#"# .DS_Store files!
.DS_Store

# Build output directory
build/
"#;

/// Import an existing package into a project directory
pub fn import_package(pkg_path: &Path, project_dir: &Path, format: OutputFormat) -> Result<()> {
    // Validate package exists
    if !pkg_path.exists() {
        return Err(crate::errors::import_failed(format!(
            "Package not found: {}",
            pkg_path.display()
        ))
        .into());
    }

    // Check if project directory already exists
    if project_dir.exists() {
        return Err(crate::errors::project_exists(format!(
            "Project directory already exists: {}",
            project_dir.display()
        ))
        .into());
    }

    println!("Importing package: {}", pkg_path.display());

    // Create temp directory for expansion
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;
    let expanded_dir = temp_dir.path().join("expanded");

    // Expand the package
    println!("Expanding package...");
    pkgutil_expand(pkg_path, &expanded_dir)?;

    // Detect package type and find component
    let component_dir = find_package_component(&expanded_dir)?;

    // Create project directory structure
    fs::create_dir_all(project_dir)?;

    let payload_dir = project_dir.join("payload");
    fs::create_dir_all(&payload_dir)?;

    let scripts_dir = project_dir.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    // Extract payload
    println!("Extracting payload...");
    extract_payload(&component_dir, &payload_dir)?;

    // Extract scripts
    println!("Extracting scripts...");
    extract_scripts(&component_dir, &scripts_dir)?;

    // Extract BOM info
    println!("Extracting BOM info...");
    let bom_txt = extract_bom_info(&component_dir, project_dir)?;

    // Apply BOM permissions to payload
    if let Some(bom_path) = bom_txt {
        println!("Applying file permissions from BOM...");
        apply_bom_to_payload(&bom_path, &payload_dir)?;
    }

    // Extract build info from PackageInfo or Info.plist
    println!("Creating build-info...");
    let build_info = extract_build_info(&component_dir, pkg_path)?;
    build_info.save(project_dir, format)?;

    // Create .gitignore
    let gitignore_path = project_dir.join(".gitignore");
    fs::write(&gitignore_path, DEFAULT_GITIGNORE)?;

    println!(
        "\nPackage imported successfully to: {}",
        project_dir.display()
    );
    println!("\nProject structure created:");
    println!("  {}/", project_dir.display());
    println!("    ├── {}", format.filename());
    println!("    ├── payload/");
    println!("    ├── scripts/");
    println!("    ├── Bom.txt");
    println!("    └── .gitignore");

    Ok(())
}

/// Find the package component directory (for flat packages)
fn find_package_component(expanded_dir: &Path) -> Result<std::path::PathBuf> {
    // Look for a .pkg directory inside (flat package structure)
    for entry in fs::read_dir(expanded_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if name.ends_with(".pkg") {
                return Ok(path);
            }
        }
    }

    // If no .pkg subdirectory, assume expanded_dir is the component
    Ok(expanded_dir.to_path_buf())
}

/// Extract payload from expanded package
fn extract_payload(component_dir: &Path, payload_dir: &Path) -> Result<()> {
    // Try different payload archive formats
    let payload_archives = ["Payload", "Payload.cpio.gz", "Archive.pax.gz"];

    for archive_name in payload_archives {
        let archive_path = component_dir.join(archive_name);
        if archive_path.exists() {
            // Use ditto to extract
            ditto_extract(&archive_path, payload_dir)?;
            return Ok(());
        }
    }

    // No payload found - this might be a nopayload package
    println!("Note: No payload archive found (may be a scripts-only package)");
    Ok(())
}

/// Extract scripts from expanded package
fn extract_scripts(component_dir: &Path, scripts_dir: &Path) -> Result<()> {
    // Scripts are typically in Resources or Scripts directory
    let script_sources = ["Resources", "Scripts"];

    for source in script_sources {
        let source_dir = component_dir.join(source);
        if source_dir.exists() && source_dir.is_dir() {
            // Copy preinstall and postinstall if they exist
            for script_name in ["preinstall", "postinstall"] {
                let script_path = source_dir.join(script_name);
                if script_path.exists() {
                    let dest = scripts_dir.join(script_name);
                    fs::copy(&script_path, &dest)?;
                }
            }
        }
    }

    Ok(())
}

/// Extract BOM info and save to Bom.txt
fn extract_bom_info(
    component_dir: &Path,
    project_dir: &Path,
) -> Result<Option<std::path::PathBuf>> {
    // Look for Bom file
    let bom_names = ["Bom", "Contents/Archive.bom"];

    for bom_name in bom_names {
        let bom_path = component_dir.join(bom_name);
        if bom_path.exists() {
            let bom_content = lsbom_extract(&bom_path)?;
            let bom_txt = project_dir.join("Bom.txt");
            fs::write(&bom_txt, &bom_content)?;
            return Ok(Some(bom_txt));
        }
    }

    // Also check for PackageInfo.bom
    for entry in WalkDir::new(component_dir)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if name == "Bom" || name.ends_with(".bom") {
            let bom_content = lsbom_extract(entry.path())?;
            let bom_txt = project_dir.join("Bom.txt");
            fs::write(&bom_txt, &bom_content)?;
            return Ok(Some(bom_txt));
        }
    }

    println!("Note: No BOM file found in package");
    Ok(None)
}

/// Extract build info from PackageInfo or Info.plist
fn extract_build_info(component_dir: &Path, pkg_path: &Path) -> Result<BuildInfo> {
    let mut build_info = BuildInfo::default();

    // Get package name from filename
    let pkg_name = pkg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Package");
    build_info.name = format!("{}-${{version}}.pkg", pkg_name);

    // Try to read PackageInfo
    let package_info_path = component_dir.join("PackageInfo");
    if package_info_path.exists()
        && let Ok(content) = fs::read_to_string(&package_info_path)
    {
        // Parse basic info from PackageInfo XML
        if let Some(id) = extract_xml_attribute(&content, "identifier") {
            build_info.identifier = id;
        }
        if let Some(version) = extract_xml_attribute(&content, "version") {
            build_info.version = version;
        }
        if let Some(location) = extract_xml_attribute(&content, "install-location") {
            build_info.install_location = location;
        }
    }

    // Try Info.plist as fallback
    let info_plist_path = component_dir.join("Info.plist");
    if info_plist_path.exists()
        && build_info.identifier.is_empty()
        && let Ok(info) = plist::from_file::<_, plist::Dictionary>(&info_plist_path)
    {
        if let Some(id) = info.get("CFBundleIdentifier").and_then(|v| v.as_string()) {
            build_info.identifier = id.to_string();
        }
        if let Some(version) = info.get("CFBundleVersion").and_then(|v| v.as_string()) {
            build_info.version = version.to_string();
        }
    }

    // Set default identifier if still empty
    if build_info.identifier.is_empty() {
        build_info.identifier = format!("com.github.munki.pkg.{}", pkg_name);
    }

    Ok(build_info)
}

/// Extract an attribute value from simple XML
fn extract_xml_attribute(content: &str, attr_name: &str) -> Option<String> {
    let pattern = format!(r#"{}="([^"]+)""#, attr_name);
    let re = regex_lite::Regex::new(&pattern).ok()?;
    re.captures(content)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_xml_attribute() {
        let xml = r#"<pkg-info identifier="com.example.pkg" version="1.0">"#;
        assert_eq!(
            extract_xml_attribute(xml, "identifier"),
            Some("com.example.pkg".to_string())
        );
        assert_eq!(
            extract_xml_attribute(xml, "version"),
            Some("1.0".to_string())
        );
        assert_eq!(extract_xml_attribute(xml, "missing"), None);
    }
}
