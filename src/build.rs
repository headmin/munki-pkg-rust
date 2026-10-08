//! Package building functionality
//!
//! Implements the core package building workflow using macOS
//! pkgbuild and productbuild tools.
//!
//! Original algorithm by Greg Neagle in munki-pkg.

use crate::build_result::BuildResult;
use crate::config::{BuildInfo, NotarizationAuth, PostinstallAction};
use crate::errors::{build_failed, invalid_config, notarization_failed, signing_failed};
use crate::external::{
    self, PKGBUILD, PRODUCTBUILD, ditto_copy, lsbom_extract, productsign, run_command_checked,
    stapler_staple,
};
use crate::project::{
    get_payload_dir, get_scripts_dir, has_payload, has_scripts, validate_project,
};
use crate::provenance::Provenance;
use crate::verify::verify_package;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use walkdir::WalkDir;

/// Load environment variables from a .env file
pub fn load_env_file(project_dir: &Path, quiet: bool) -> Result<()> {
    // Try project_dir/.env first, then project_dir/../.env
    let env_paths = [
        project_dir.join(".env"),
        project_dir
            .parent()
            .map(|p| p.join(".env"))
            .unwrap_or_default(),
    ];

    for env_path in env_paths {
        if env_path.exists() {
            if !quiet {
                println!("Loading environment from: {}", env_path.display());
            }

            let file = File::open(&env_path)
                .with_context(|| format!("Failed to open {}", env_path.display()))?;

            for line in BufReader::new(file).lines() {
                let line = line?;
                let line = line.trim();

                // Skip comments and empty lines
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                // Parse KEY=VALUE
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');

                    // Only set if not already in environment
                    if std::env::var(key).is_err() {
                        // SAFETY: Called early in single-threaded startup
                        unsafe { std::env::set_var(key, value) };
                    }
                }
            }

            return Ok(());
        }
    }

    Ok(())
}

/// Everything that varies between two builds of the same project.
///
/// Grouped into a struct rather than passed as a dozen positional booleans,
/// which is how `skip_signing` and `skip_notarization` end up swapped.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Write `Bom.txt` back to the project after building.
    pub export_bom: bool,
    /// Suppress human-readable status output.
    pub quiet: bool,
    /// Do not sign, even when `signing_info` is configured.
    pub skip_signing: bool,
    /// Do not notarize, even when `notarization_info` is configured.
    pub skip_notarization: bool,
    /// Notarize but do not staple the ticket.
    pub skip_stapling: bool,
    /// Write a `<pkg>.provenance.json` attestation beside the package.
    pub provenance: bool,
    /// Verify the finished package against build-info before reporting success.
    pub verify: bool,
    /// Replace the build-info `version`, e.g. from a git tag or CI variable.
    pub version_override: Option<String>,
    /// Write the package here instead of the project's `build/` directory.
    pub output_dir: Option<PathBuf>,
}

/// Build a package from a project directory.
///
/// Returns a [`BuildResult`] describing what was actually produced — its
/// `signed`, `notarized` and `stapled` flags report what happened, not what
/// build-info asked for.
pub fn build_package(project_dir: &Path, options: &BuildOptions) -> Result<BuildResult> {
    let quiet = options.quiet;

    // Validate project structure
    validate_project(project_dir)?;

    // Load .env file if present (for op://, credentials, etc.)
    load_env_file(project_dir, quiet)?;

    // Verify required tools are available
    external::verify_tools()?;

    // Load build info, applying --pkg-version and stamping dynamic date tokens
    // before anything reads the version.
    let build_info = BuildInfo::load_resolved(project_dir, options.version_override.as_deref())?;
    build_info.validate()?;

    if !quiet {
        println!("Building package: {}", build_info.resolved_name());
    }

    // Create temporary working directory
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;

    // Create build output directory — the project's build/ unless redirected
    let build_dir = match &options.output_dir {
        Some(dir) => dir.clone(),
        None => project_dir.join("build"),
    };
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("Failed to create build directory: {}", build_dir.display()))?;

    // Determine if we have payload and/or scripts
    let has_payload_content = has_payload(project_dir);
    let has_script_content = has_scripts(project_dir);

    if !has_payload_content && !has_script_content {
        return Err(
            invalid_config("Project has no payload and no scripts. Nothing to build.").into(),
        );
    }

    // Prepare payload directory (clean .DS_Store files)
    let working_payload = if has_payload_content {
        let payload_dir = get_payload_dir(project_dir).unwrap();
        let working = temp_dir.path().join("payload");
        ditto_copy(&payload_dir, &working)?;
        clean_ds_store(&working)?;
        // Quarantine/provenance must not ship inside the package; opt out
        // with preserve_xattr.
        if !build_info.preserve_xattr {
            clear_xattrs(&working)?;
        }
        Some(working)
    } else {
        None
    };

    // Prepare scripts directory
    let working_scripts = if has_script_content {
        let scripts_dir = get_scripts_dir(project_dir).unwrap();
        let working = temp_dir.path().join("scripts");
        ditto_copy(&scripts_dir, &working)?;
        clean_ds_store(&working)?;
        clear_xattrs(&working)?;
        make_scripts_executable(&working)?;
        Some(working)
    } else {
        None
    };

    // Create component property list if we have payload
    let component_plist = if let Some(ref payload) = working_payload {
        let plist_path = temp_dir.path().join("component.plist");
        external::pkgbuild_analyze(payload, &plist_path)?;

        // Modify component plist to suppress bundle relocation if needed
        if build_info.suppress_bundle_relocation {
            suppress_bundle_relocation(&plist_path)?;
        }

        Some(plist_path)
    } else {
        None
    };

    // Create PackageInfo if needed for postinstall action
    let pkg_info_path = if build_info.postinstall_action != PostinstallAction::None {
        let path = temp_dir.path().join("PackageInfo");
        create_package_info(&path, &build_info)?;
        Some(path)
    } else {
        None
    };

    // Build the package
    let unsigned_pkg = temp_dir.path().join("unsigned.pkg");
    build_pkg(
        &build_info,
        working_payload.as_deref(),
        working_scripts.as_deref(),
        component_plist.as_deref(),
        pkg_info_path.as_deref(),
        &unsigned_pkg,
        quiet,
    )?;

    // Convert to distribution package if needed
    let pre_sign_pkg = if build_info.distribution_style {
        let dist_pkg = temp_dir.path().join("distribution.pkg");
        build_distribution_pkg(&build_info, &unsigned_pkg, &dist_pkg, quiet)?;
        dist_pkg
    } else {
        unsigned_pkg
    };

    // Determine final output path
    let output_pkg = build_dir.join(build_info.resolved_name());

    // Sign package if configured and not skipped
    let mut signed = false;
    if let Some(ref signing_info) = build_info.signing_info {
        if options.skip_signing {
            if !quiet {
                println!("Skipping signing as requested");
            }
            fs::copy(&pre_sign_pkg, &output_pkg)?;
        } else {
            signed = true;
            // Use installer_identity for pkg signing, or derive from identity
            let pkg_identity = signing_info
                .installer_identity
                .as_deref()
                .unwrap_or_else(|| {
                    // Try to derive installer identity from application identity
                    // "Developer ID Application: Name" -> "Developer ID Installer: Name"
                    &signing_info.identity
                });

            // Check if we have an installer identity (not application)
            if pkg_identity.contains("Application") {
                if !quiet {
                    println!(
                        "Warning: No installer identity configured. Attempting to derive from application identity."
                    );
                }
                // Try to use the derived installer identity
                let derived = pkg_identity.replace("Application", "Installer");
                if !quiet {
                    println!("Signing package with identity: {}", derived);
                }
                productsign(
                    &pre_sign_pkg,
                    &output_pkg,
                    &derived,
                    signing_info.keychain.as_deref(),
                    signing_info.timestamp,
                )
                .map_err(|error| signing_failed(format!("Failed to sign package: {:#}", error)))?;
            } else {
                if !quiet {
                    println!("Signing package with identity: {}", pkg_identity);
                }
                productsign(
                    &pre_sign_pkg,
                    &output_pkg,
                    pkg_identity,
                    signing_info.keychain.as_deref(),
                    signing_info.timestamp,
                )
                .map_err(|error| signing_failed(format!("Failed to sign package: {:#}", error)))?;
            }
        }
    } else {
        fs::copy(&pre_sign_pkg, &output_pkg)?;
    }

    // Notarize if configured and not skipped
    let mut notarized = false;
    let mut stapled = false;
    if let Some(ref notarization_info) = build_info.notarization_info {
        if options.skip_notarization {
            if !quiet {
                println!("Skipping notarization as requested");
            }
        } else {
            if !quiet {
                println!("Submitting package for notarization...");
            }
            stapled =
                notarize_package(&output_pkg, notarization_info, options.skip_stapling, quiet)?;
            notarized = true;
        }
    }

    // Export BOM info if requested
    if options.export_bom {
        export_bom_info(&output_pkg, project_dir, quiet)?;
    }

    // Verify the finished artifact actually matches what build-info declared.
    // Runs before the result is reported, so a mismatch fails the build.
    if options.verify {
        if !quiet {
            println!("Verifying package...");
        }
        let expected_identifier = if build_info.distribution_style {
            build_info
                .product_id
                .as_deref()
                .unwrap_or(&build_info.identifier)
        } else {
            &build_info.identifier
        };
        verify_package(
            &output_pkg,
            expected_identifier,
            &build_info.version,
            signed,
            notarized,
            quiet,
        )?;
    }

    let result = BuildResult::new(
        build_info.resolved_name(),
        build_info.version.clone(),
        build_info.identifier.clone(),
        &output_pkg,
        signed,
        notarized,
        stapled,
    )?;

    if options.provenance {
        let provenance = Provenance::build(&build_info, &output_pkg, project_dir)?;
        let sidecar = provenance.write_sidecar(&output_pkg)?;
        if !quiet {
            println!("Wrote provenance: {}", sidecar.display());
        }
    }

    if !quiet {
        println!("\nPackage built successfully: {}", output_pkg.display());
    }

    Ok(result)
}

/// Build the component package using pkgbuild
fn build_pkg(
    build_info: &BuildInfo,
    payload_dir: Option<&Path>,
    scripts_dir: Option<&Path>,
    component_plist: Option<&Path>,
    pkg_info: Option<&Path>,
    output: &Path,
    quiet: bool,
) -> Result<()> {
    let mut cmd = Command::new(PKGBUILD);

    // Add root directory or nopayload
    if let Some(payload) = payload_dir {
        cmd.arg("--root").arg(payload);
    } else {
        cmd.arg("--nopayload");
    }

    // Add component plist
    if let Some(plist) = component_plist {
        cmd.arg("--component-plist").arg(plist);
    }

    // Add scripts
    if let Some(scripts) = scripts_dir {
        cmd.arg("--scripts").arg(scripts);
    }

    // Add PackageInfo
    if let Some(info) = pkg_info {
        cmd.arg("--info").arg(info);
    }

    // Add identifier and version
    cmd.arg("--identifier").arg(&build_info.identifier);
    cmd.arg("--version").arg(&build_info.version);

    // Add install location
    cmd.arg("--install-location")
        .arg(&build_info.install_location);

    // Add ownership
    cmd.arg("--ownership").arg(build_info.ownership.as_str());

    // Add compression if specified
    if let Some(ref compression) = build_info.compression {
        cmd.arg("--compression").arg(compression.as_str());
    }

    // Add min-os-version if specified
    if let Some(ref min_os) = build_info.min_os_version {
        cmd.arg("--min-os-version").arg(min_os);
    }

    // Add large-payload if enabled
    if build_info.large_payload {
        cmd.arg("--large-payload");
    }

    // Add preserve-xattr if enabled
    if build_info.preserve_xattr {
        cmd.arg("--preserve-xattr");
    }

    // Add output path
    cmd.arg(output);

    if !quiet {
        println!("Running pkgbuild...");
    }

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Build a distribution-style package using productbuild
fn build_distribution_pkg(
    build_info: &BuildInfo,
    component_pkg: &Path,
    output: &Path,
    quiet: bool,
) -> Result<()> {
    let mut cmd = Command::new(PRODUCTBUILD);

    cmd.arg("--package").arg(component_pkg);

    // Use product id or identifier
    let product_id = build_info
        .product_id
        .as_ref()
        .unwrap_or(&build_info.identifier);
    cmd.arg("--identifier").arg(product_id);
    cmd.arg("--version").arg(&build_info.version);

    cmd.arg(output);

    if !quiet {
        println!("Running productbuild for distribution package...");
    }

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Create PackageInfo XML file for postinstall actions
fn create_package_info(path: &Path, build_info: &BuildInfo) -> Result<()> {
    let action = match build_info.postinstall_action {
        PostinstallAction::Logout => "logout",
        PostinstallAction::Restart => "restart",
        PostinstallAction::None => return Ok(()),
    };

    let content = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<pkg-info postinstall-action="{}"/>
"#,
        action
    );

    let mut file = File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

/// Suppress bundle relocation in component plist
fn suppress_bundle_relocation(plist_path: &Path) -> Result<()> {
    let data = fs::read(plist_path)?;
    let mut components: Vec<plist::Dictionary> = plist::from_bytes(&data)?;

    for component in &mut components {
        component.insert(
            "BundleIsRelocatable".to_string(),
            plist::Value::Boolean(false),
        );
    }

    let mut file = File::create(plist_path)?;
    plist::to_writer_xml(&mut file, &components)?;
    Ok(())
}

/// Recursively clear extended attributes (quarantine, provenance, ...)
fn clear_xattrs(dir: &Path) -> Result<()> {
    run_command_checked(Command::new("/usr/bin/xattr").arg("-cr").arg(dir))?;
    Ok(())
}

/// Remove .DS_Store files from directory tree
fn clean_ds_store(dir: &Path) -> Result<()> {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_name() == ".DS_Store" {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Make scripts executable (mode 755)
fn make_scripts_executable(scripts_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(scripts_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms)?;
        }
    }
    Ok(())
}

/// Resolve a value that might be an op:// URL, @keychain:, or @env: reference
pub fn resolve_secret(value: Option<&str>, quiet: bool) -> Result<Option<String>> {
    let Some(val) = value else {
        return Ok(None);
    };

    if val.starts_with("op://") {
        // 1Password CLI
        if !quiet {
            println!("Resolving 1Password reference...");
        }
        let output = std::process::Command::new("op")
            .args(["read", val])
            .output()
            .context("Failed to run 'op' CLI. Is 1Password CLI installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(
                invalid_config(format!("Failed to read from 1Password: {}", stderr)).into(),
            );
        }

        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else if val.starts_with("@keychain:") {
        // macOS Keychain - pass through for notarytool to handle
        Ok(Some(val.to_string()))
    } else if let Some(var_name) = val.strip_prefix("@env:") {
        // Environment variable (can be loaded from .env file)
        let resolved = std::env::var(var_name)
            .with_context(|| format!("Environment variable {} not set", var_name))?;
        Ok(Some(resolved))
    } else {
        // Plain value
        Ok(Some(val.to_string()))
    }
}

/// Decide how a submission authenticates, resolving any secret references.
///
/// A keychain profile wins when set: it is the only method that keeps
/// credentials out of both build-info and the environment.
pub fn resolve_notarization_auth(
    info: &crate::config::NotarizationInfo,
    quiet: bool,
) -> Result<NotarizationAuth> {
    if let Some(profile) = resolve_secret(info.keychain_profile.as_deref(), quiet)? {
        return Ok(NotarizationAuth::KeychainProfile(profile));
    }

    let apple_id = resolve_secret(info.apple_id.as_deref(), quiet)?;
    let password = resolve_secret(info.password.as_deref(), quiet)?;
    let team_id = resolve_secret(info.team_id.as_deref(), quiet)?;
    if let (Some(apple_id), Some(password), Some(team_id)) = (apple_id, password, team_id) {
        return Ok(NotarizationAuth::AppleId {
            apple_id,
            password,
            team_id,
        });
    }

    let key_path = resolve_secret(info.api_key_path.as_deref(), quiet)?;
    let key_id = resolve_secret(info.api_key_id.as_deref(), quiet)?;
    let issuer_id = resolve_secret(info.api_issuer_id.as_deref(), quiet)?;
    if let (Some(key_path), Some(key_id), Some(issuer_id)) = (key_path, key_id, issuer_id) {
        return Ok(NotarizationAuth::ApiKey {
            key_path,
            key_id,
            issuer_id,
        });
    }

    Err(invalid_config(
        "notarization_info has no usable credentials. Set keychain_profile (see \
         `munkipkg configure`), or apple_id with team_id and password, or an API key.",
    )
    .into())
}

/// Notarize a package. Returns whether the ticket was stapled.
pub fn notarize_package(
    pkg_path: &Path,
    info: &crate::config::NotarizationInfo,
    skip_stapling: bool,
    quiet: bool,
) -> Result<bool> {
    let auth = resolve_notarization_auth(info, quiet)?;

    if !quiet {
        println!("Authenticating with {}", auth.description());
    }

    let result = external::notarytool_submit(pkg_path, &auth.to_args())
        .map_err(|error| notarization_failed(format!("Notarization upload failed: {:#}", error)))?;

    if !quiet {
        println!("Notarization result: {}", result);
    }

    // Staple the notarization ticket
    if skip_stapling {
        if !quiet {
            println!("Skipping stapling as requested");
        }
        return Ok(false);
    }

    if !quiet {
        println!("Stapling notarization ticket...");
    }
    stapler_staple(pkg_path)
        .map_err(|error| notarization_failed(format!("Stapling failed: {:#}", error)))?;

    Ok(true)
}

/// Export BOM info to Bom.txt
fn export_bom_info(pkg_path: &Path, project_dir: &Path, quiet: bool) -> Result<()> {
    // Expand package to temp directory
    let temp_dir = TempDir::new()?;
    let expanded = temp_dir.path().join("expanded");
    external::pkgutil_expand(pkg_path, &expanded)?;

    // Find Bom file
    let bom_path = find_bom_file(&expanded)?;

    // Extract BOM info
    let bom_content = lsbom_extract(&bom_path)?;

    // Write to Bom.txt
    let bom_txt = project_dir.join("Bom.txt");
    fs::write(&bom_txt, bom_content)?;

    if !quiet {
        println!("Exported BOM info to: {}", bom_txt.display());
    }

    Ok(())
}

/// Find the Bom file in an expanded package
fn find_bom_file(expanded_dir: &Path) -> Result<PathBuf> {
    for entry in WalkDir::new(expanded_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if name == "Bom" || name.ends_with(".bom") {
            return Ok(entry.path().to_path_buf());
        }
    }
    Err(build_failed("No Bom file found in expanded package").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_clean_ds_store() {
        let temp = TempDir::new().unwrap();
        let ds_store = temp.path().join(".DS_Store");
        fs::write(&ds_store, "test").unwrap();

        assert!(ds_store.exists());
        clean_ds_store(temp.path()).unwrap();
        assert!(!ds_store.exists());
    }

    #[test]
    fn test_create_package_info() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("PackageInfo");

        let build_info = BuildInfo {
            postinstall_action: PostinstallAction::Restart,
            ..Default::default()
        };

        create_package_info(&path, &build_info).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("postinstall-action=\"restart\""));
    }
}
