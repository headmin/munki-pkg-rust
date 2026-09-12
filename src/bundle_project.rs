//! Bundle project scaffolding
//!
//! Creates the directory structure for a new multi-component
//! distribution bundle project.

use crate::config::{BundleInfo, ComponentRef, OutputFormat};
use anyhow::{Result, bail};
use std::fs;
use std::path::Path;

/// Create a new bundle project with default structure
pub fn create_bundle_project(bundle_dir: &Path, format: OutputFormat, force: bool) -> Result<()> {
    // Check if directory already exists
    if bundle_dir.exists() && !force {
        bail!(
            "Bundle directory already exists: {}. Use --force to overwrite.",
            bundle_dir.display()
        );
    }

    let bundle_name = bundle_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "my-bundle".to_string());

    // Create directories
    fs::create_dir_all(bundle_dir.join("components"))?;
    fs::create_dir_all(bundle_dir.join("build"))?;

    // Write default bundle-info
    let bundle_info = BundleInfo {
        name: format!("{}-${{version}}.pkg", bundle_name),
        version: "1.0.0".to_string(),
        identifier: format!("com.example.{}", bundle_name),
        min_os_version: Some("14.0".to_string()),
        install_location: "/".to_string(),
        components: vec![ComponentRef {
            name: "example".to_string(),
        }],
        signing_info: None,
        notarization_info: None,
    };
    bundle_info.save(bundle_dir, format)?;

    // Write .gitignore
    fs::write(bundle_dir.join(".gitignore"), "build/\n.DS_Store\n.env\n")?;

    println!("Bundle project created at: {}", bundle_dir.display());
    println!("\nStructure:");
    println!("  {}/", bundle_name);
    println!("  ├── {}", format.bundle_filename());
    println!("  ├── components/     (add munkipkg sub-projects here)");
    println!("  ├── build/          (output directory)");
    println!("  └── .gitignore");
    println!("\nNext steps:");
    println!("  1. Create component sub-projects under components/");
    println!(
        "     e.g. munkipkg create {}/components/my-app",
        bundle_dir.display()
    );
    println!("  2. Add payload files to each component");
    println!(
        "  3. Update {} with component names",
        format.bundle_filename()
    );
    println!("  4. Run: munkipkg bundle build {}", bundle_dir.display());

    Ok(())
}

/// Validate that a directory is a valid bundle project
pub fn validate_bundle_project(bundle_dir: &Path) -> Result<()> {
    if !bundle_dir.exists() {
        bail!("Bundle directory does not exist: {}", bundle_dir.display());
    }

    if !bundle_dir.is_dir() {
        bail!("Not a directory: {}", bundle_dir.display());
    }

    // Check for bundle-info file
    let has_bundle_info = [
        "bundle-info.plist",
        "bundle-info.json",
        "bundle-info.yaml",
        "bundle-info.toml",
    ]
    .iter()
    .any(|f| bundle_dir.join(f).exists());

    if !has_bundle_info {
        bail!(
            "No bundle-info file found in {}. Is this a bundle project?",
            bundle_dir.display()
        );
    }

    // Check for components directory
    let components_dir = bundle_dir.join("components");
    if !components_dir.exists() {
        bail!("No components/ directory found in {}", bundle_dir.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_bundle_project() {
        let temp = TempDir::new().unwrap();
        let bundle_dir = temp.path().join("test-bundle");

        create_bundle_project(&bundle_dir, OutputFormat::Toml, false).unwrap();

        assert!(bundle_dir.join("bundle-info.toml").exists());
        assert!(bundle_dir.join("components").is_dir());
        assert!(bundle_dir.join("build").is_dir());
        assert!(bundle_dir.join(".gitignore").exists());

        // Verify bundle-info loads correctly
        let info = BundleInfo::load(&bundle_dir).unwrap();
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.identifier, "com.example.test-bundle");
        assert_eq!(info.components.len(), 1);
    }

    #[test]
    fn test_create_bundle_project_exists_no_force() {
        let temp = TempDir::new().unwrap();
        let bundle_dir = temp.path().join("existing");
        fs::create_dir_all(&bundle_dir).unwrap();

        let result = create_bundle_project(&bundle_dir, OutputFormat::Toml, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_bundle_project_exists_with_force() {
        let temp = TempDir::new().unwrap();
        let bundle_dir = temp.path().join("existing");
        fs::create_dir_all(&bundle_dir).unwrap();

        create_bundle_project(&bundle_dir, OutputFormat::Toml, true).unwrap();
        assert!(bundle_dir.join("bundle-info.toml").exists());
    }

    #[test]
    fn test_validate_bundle_project() {
        let temp = TempDir::new().unwrap();
        let bundle_dir = temp.path().join("valid-bundle");

        create_bundle_project(&bundle_dir, OutputFormat::Toml, false).unwrap();
        assert!(validate_bundle_project(&bundle_dir).is_ok());
    }

    #[test]
    fn test_validate_bundle_project_missing() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("nope");
        assert!(validate_bundle_project(&missing).is_err());
    }

    #[test]
    fn test_validate_bundle_project_no_config() {
        let temp = TempDir::new().unwrap();
        let bad = temp.path().join("bad");
        fs::create_dir_all(bad.join("components")).unwrap();
        assert!(validate_bundle_project(&bad).is_err());
    }
}
