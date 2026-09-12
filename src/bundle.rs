//! Bundle building — multi-component distribution packages
//!
//! Orchestrates building multiple munkipkg sub-projects into a single
//! signed and notarized distribution package using `productbuild --distribution`.

use crate::build::{BuildOptions, build_package, load_env_file, notarize_package};
use crate::bundle_project::validate_bundle_project;
use crate::config::{BuildInfo, BundleInfo};
use crate::distribution::{ComponentPackage, generate_distribution_xml};
use crate::external::{self, productbuild_distribution, productsign};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Build a distribution bundle from component packages
pub fn build_bundle(
    bundle_dir: &Path,
    quiet: bool,
    skip_signing: bool,
    skip_notarization: bool,
    skip_stapling: bool,
) -> Result<()> {
    // Validate bundle project structure
    validate_bundle_project(bundle_dir)?;

    // Load .env from bundle dir or parent
    load_env_file(bundle_dir, quiet)?;

    // Verify required macOS tools
    external::verify_tools()?;

    // Load and validate bundle config
    let bundle_info = BundleInfo::load(bundle_dir)?;
    bundle_info.validate()?;

    if !quiet {
        println!("Building bundle: {}", bundle_info.resolved_name());
        println!(
            "Components: {}",
            bundle_info
                .components
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let components_dir = bundle_dir.join("components");

    // Build each component and collect metadata
    let mut component_packages: Vec<ComponentPackage> = Vec::new();
    let mut built_pkg_paths: Vec<PathBuf> = Vec::new();

    for comp in &bundle_info.components {
        let project_dir = components_dir.join(&comp.name);

        if !project_dir.exists() {
            bail!(
                "Component project not found: {}. Expected at {}",
                comp.name,
                project_dir.display()
            );
        }

        if !quiet {
            println!("\n--- Building component: {} ---", comp.name);
        }

        // Build component (skip signing/notarization — handled at bundle level)
        // Components are built bare: the bundle is signed and notarized as
        // a whole, so a component signature would only be replaced.
        build_package(
            &project_dir,
            &BuildOptions {
                quiet,
                skip_signing: true,
                skip_notarization: true,
                skip_stapling: true,
                ..Default::default()
            },
        )?;

        // Load component's build-info to get identifier, version, pkg name
        let build_info = BuildInfo::load(&project_dir)?;
        let pkg_name = build_info.resolved_name();
        let pkg_path = project_dir.join("build").join(&pkg_name);

        if !pkg_path.exists() {
            bail!(
                "Component build produced no output: expected {}",
                pkg_path.display()
            );
        }

        component_packages.push(ComponentPackage {
            name: comp.name.clone(),
            identifier: build_info.identifier.clone(),
            version: build_info.version.clone(),
            pkg_filename: pkg_name.clone(),
        });
        built_pkg_paths.push(pkg_path);
    }

    if !quiet {
        println!("\n--- Assembling distribution package ---");
    }

    // Create temp directory for assembly
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;

    // Copy all component pkgs into a flat packages/ directory
    let packages_dir = temp_dir.path().join("packages");
    fs::create_dir_all(&packages_dir)?;

    for pkg_path in &built_pkg_paths {
        let filename = pkg_path.file_name().unwrap();
        let dest = packages_dir.join(filename);
        fs::copy(pkg_path, &dest)
            .with_context(|| format!("Failed to copy {} to staging area", pkg_path.display()))?;
    }

    // Generate distribution.xml
    let dist_xml_path = temp_dir.path().join("distribution.xml");
    let dist_xml = generate_distribution_xml(&bundle_info, &component_packages);
    fs::write(&dist_xml_path, &dist_xml)?;

    if !quiet {
        println!("Generated distribution.xml");
    }

    // Run productbuild --distribution
    let unsigned_pkg = temp_dir.path().join("unsigned.pkg");
    productbuild_distribution(&dist_xml_path, &packages_dir, &unsigned_pkg)?;

    if !quiet {
        println!("Built distribution package");
    }

    // Create build output directory
    let build_dir = bundle_dir.join("build");
    fs::create_dir_all(&build_dir)?;
    let output_pkg = build_dir.join(bundle_info.resolved_name());

    // Sign if configured
    if let Some(ref signing_info) = bundle_info.signing_info {
        if skip_signing {
            if !quiet {
                println!("Skipping signing as requested");
            }
            fs::copy(&unsigned_pkg, &output_pkg)?;
        } else {
            let pkg_identity = signing_info
                .installer_identity
                .as_deref()
                .unwrap_or(&signing_info.identity);

            // Derive installer identity if needed
            let effective_identity = if pkg_identity.contains("Application") {
                let derived = pkg_identity.replace("Application", "Installer");
                if !quiet {
                    println!("Signing bundle with derived identity: {}", derived);
                }
                derived
            } else {
                if !quiet {
                    println!("Signing bundle with identity: {}", pkg_identity);
                }
                pkg_identity.to_string()
            };

            productsign(
                &unsigned_pkg,
                &output_pkg,
                &effective_identity,
                signing_info.keychain.as_deref(),
                signing_info.timestamp,
            )?;
        }
    } else {
        fs::copy(&unsigned_pkg, &output_pkg)?;
    }

    // Notarize if configured
    if let Some(ref notarization_info) = bundle_info.notarization_info {
        if skip_notarization {
            if !quiet {
                println!("Skipping notarization as requested");
            }
        } else {
            if !quiet {
                println!("Submitting bundle for notarization...");
            }
            notarize_package(&output_pkg, notarization_info, skip_stapling, quiet)?;
        }
    }

    if !quiet {
        println!("\nBundle built successfully: {}", output_pkg.display());
        println!(
            "Contains {} component{}:",
            component_packages.len(),
            if component_packages.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for comp in &component_packages {
            println!("  - {} ({} v{})", comp.name, comp.identifier, comp.version);
        }
    }

    Ok(())
}
