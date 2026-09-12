//! Signing configuration functionality
//!
//! Scans for available signing identities and configures
//! signing and notarization settings for projects.

use crate::config::{BuildInfo, NotarizationInfo, OutputFormat, SigningInfo};
use crate::external;
use anyhow::{Context, Result, bail};
use inquire::{Confirm, Select, Text};
use std::path::Path;
use std::process::Command;

/// Scan for available Developer ID Application certificates
pub fn scan_application_identities() -> Result<Vec<String>> {
    scan_identities("Application")
}

/// Scan for available Developer ID Installer certificates
#[allow(dead_code)]
pub fn scan_installer_identities() -> Result<Vec<String>> {
    scan_identities("Installer")
}

/// Scan for signing identities of a specific type
fn scan_identities(identity_type: &str) -> Result<Vec<String>> {
    // For Application identities, use codesigning policy
    // For Installer identities, scan all identities (no policy filter)
    let output = if identity_type == "Application" {
        Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run security command")?
    } else {
        // Installer certificates don't have codesigning policy
        Command::new("security")
            .args(["find-identity", "-v"])
            .output()
            .context("Failed to run security command")?
    };

    if !output.status.success() {
        bail!("Failed to list signing identities");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pattern = format!("Developer ID {}", identity_type);

    let identities: Vec<String> = stdout
        .lines()
        .filter(|line| line.contains(&pattern))
        .filter_map(|line| {
            // Extract the identity name between quotes
            let start = line.find('"')?;
            let end = line.rfind('"')?;
            if start < end {
                Some(line[start + 1..end].to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(identities)
}

/// Interactively configure signing info for a project
pub fn configure_signing_interactive(project_dir: &Path) -> Result<()> {
    // Load existing build info
    let mut build_info = BuildInfo::load(project_dir)?;

    println!("\n=== Signing Configuration ===\n");

    // Check for existing config and offer targeted reconfiguration
    let has_signing = build_info.signing_info.is_some();
    let has_notarization = build_info.notarization_info.is_some();

    let (configure_app, configure_installer, configure_notarization) =
        if has_signing || has_notarization {
            // Show current configuration
            if let Some(ref signing) = build_info.signing_info {
                println!("Current signing configuration:");
                println!("  Application: {}", signing.identity);
                if let Some(ref inst) = signing.installer_identity {
                    println!("  Installer:   {}", inst);
                } else {
                    println!("  Installer:   (not configured)");
                }
            }
            if let Some(ref notarization) = build_info.notarization_info {
                if let Some(ref apple_id) = notarization.apple_id {
                    println!("  Apple ID:    {}", apple_id);
                }
                if let Some(ref team_id) = notarization.team_id {
                    println!("  Team ID:     {}", team_id);
                }
            }
            println!();

            // Let user choose what to reconfigure
            let options = vec![
                "Reconfigure all",
                "Application identity only (binary signing)",
                "Installer identity only (pkg signing)",
                "Notarization only",
                "Keep current configuration",
            ];

            let selection = Select::new("What would you like to configure?", options).prompt()?;

            match selection {
                "Reconfigure all" => (true, true, true),
                "Application identity only (binary signing)" => (true, false, false),
                "Installer identity only (pkg signing)" => (false, true, false),
                "Notarization only" => (false, false, true),
                _ => {
                    println!("Keeping existing configuration.");
                    return Ok(());
                }
            }
        } else {
            // No existing config, configure everything
            (true, true, true)
        };

    // Get existing identities for pre-selection
    let existing_app_identity = build_info.signing_info.as_ref().map(|s| s.identity.clone());
    let existing_installer_identity = build_info
        .signing_info
        .as_ref()
        .and_then(|s| s.installer_identity.clone());

    let mut app_selection = existing_app_identity.clone().unwrap_or_default();
    let mut installer_selection = existing_installer_identity.clone();

    if configure_app {
        // Scan for Application identities (for binary signing)
        let app_identities = scan_application_identities()?;

        if app_identities.is_empty() {
            println!("No Developer ID Application certificates found in Keychain.");
            println!("You need a Developer ID certificate from Apple to sign packages.");
            return Ok(());
        }

        // Find default index for Application identity
        let app_default_idx = existing_app_identity
            .as_ref()
            .and_then(|current| app_identities.iter().position(|id| id == current))
            .unwrap_or(0);

        // Let user select Application identity
        let mut app_options: Vec<&str> = app_identities.iter().map(|s| s.as_str()).collect();
        app_options.push("Skip signing configuration");

        let selection = Select::new("Application identity (for binary):", app_options)
            .with_starting_cursor(app_default_idx)
            .prompt()?;

        if selection == "Skip signing configuration" {
            println!("Skipping signing configuration.");
            return Ok(());
        }
        app_selection = selection.to_string();
    }

    if configure_installer {
        // Scan for Installer identities (for pkg signing)
        let installer_identities = scan_installer_identities()?;

        if installer_identities.is_empty() {
            println!("\nNo Developer ID Installer certificates found.");
            println!("Package signing will be skipped.\n");
            installer_selection = None;
        } else {
            // Find default index for Installer identity
            let inst_default_idx = existing_installer_identity
                .as_ref()
                .and_then(|current| installer_identities.iter().position(|id| id == current))
                .unwrap_or(0);

            let mut inst_options: Vec<&str> =
                installer_identities.iter().map(|s| s.as_str()).collect();
            inst_options.push("Skip package signing");

            let selection = Select::new("Installer identity (for pkg):", inst_options)
                .with_starting_cursor(inst_default_idx)
                .prompt()?;

            installer_selection = if selection == "Skip package signing" {
                None
            } else {
                Some(selection.to_string())
            };
        }
    }

    if configure_app || configure_installer {
        // Preserve existing signing info fields we didn't change
        let existing = build_info.signing_info.take();
        let signing_info = SigningInfo {
            identity: app_selection,
            installer_identity: installer_selection,
            keychain: existing.as_ref().and_then(|s| s.keychain.clone()),
            additional_cert_names: existing
                .as_ref()
                .and_then(|s| s.additional_cert_names.clone()),
            timestamp: existing.as_ref().map(|s| s.timestamp).unwrap_or(true),
        };
        build_info.signing_info = Some(signing_info);
    }

    if configure_notarization {
        println!("\n=== Notarization Configuration ===\n");

        // Show current notarization config if exists
        if let Some(ref notarization) = build_info.notarization_info {
            println!("Current configuration:");
            if let Some(ref apple_id) = notarization.apple_id {
                println!("  Apple ID: {}", apple_id);
            }
            if let Some(ref team_id) = notarization.team_id {
                println!("  Team ID:  {}", team_id);
            }
            if let Some(ref password) = notarization.password {
                // Mask password but show format
                let masked = if password.starts_with("op://") {
                    "op://... (1Password)".to_string()
                } else if password.starts_with("@keychain:") {
                    "@keychain:... (Keychain)".to_string()
                } else if password.starts_with("@env:") {
                    "@env:... (Environment)".to_string()
                } else {
                    "***".to_string()
                };
                println!("  Password: {}", masked);
            }
            println!();

            // Offer options when notarization already configured
            let options = vec![
                "Update notarization credentials",
                "Disable notarization (remove config)",
                "Keep current configuration",
            ];
            let selection = Select::new("Notarization:", options).prompt()?;

            match selection {
                "Update notarization credentials" => {
                    let notarization_info =
                        configure_notarization_interactive(&build_info.notarization_info)?;
                    build_info.notarization_info = Some(notarization_info);
                }
                "Disable notarization (remove config)" => {
                    build_info.notarization_info = None;
                    println!("Notarization disabled.");
                }
                _ => {
                    println!("Keeping current notarization configuration.");
                }
            }
        } else {
            // No existing config
            let do_configure = Confirm::new("Configure notarization?")
                .with_default(true)
                .with_help_message("Required for distribution outside the App Store")
                .prompt()?;

            if do_configure {
                let notarization_info =
                    configure_notarization_interactive(&build_info.notarization_info)?;
                build_info.notarization_info = Some(notarization_info);
            }
        }
    }

    // Determine format from existing file
    let format = detect_format(project_dir)?;

    // Save updated build info
    build_info.save(project_dir, format)?;

    println!("\nSigning configuration saved to {}", format.filename());

    Ok(())
}

/// Interactively configure notarization settings.
///
/// Offers three authentication methods, keychain profile first: it is the only
/// one that keeps credentials out of build-info, out of the environment, and
/// out of this process.
fn configure_notarization_interactive(
    existing: &Option<NotarizationInfo>,
) -> Result<NotarizationInfo> {
    let methods = vec![
        "Keychain profile (recommended) - credentials stay in your login keychain",
        "Apple ID + app-specific password - supports op://, @env:, @keychain:",
        "App Store Connect API key",
    ];

    let selection = Select::new("How should notarization authenticate?", methods)
        .with_help_message(
            "A keychain profile is created once per machine and reused by every project",
        )
        .prompt()?;

    let base = |info: NotarizationInfo| NotarizationInfo {
        staple_timeout: existing.as_ref().map(|n| n.staple_timeout).unwrap_or(300),
        asc_provider: existing.as_ref().and_then(|n| n.asc_provider.clone()),
        primary_bundle_id: existing.as_ref().and_then(|n| n.primary_bundle_id.clone()),
        ..info
    };

    if selection.starts_with("Keychain profile") {
        return Ok(base(configure_keychain_profile(existing)?));
    }

    if selection.starts_with("App Store Connect") {
        return Ok(base(configure_api_key(existing)?));
    }

    Ok(base(configure_apple_id(existing)?))
}

/// Configure — and if needed create — a notarytool keychain profile.
///
/// Credentials are stored once with `xcrun notarytool store-credentials`, and
/// afterwards build-info only names the profile. Nothing secret is written to
/// the project, and no environment variables are needed at build time.
fn configure_keychain_profile(existing: &Option<NotarizationInfo>) -> Result<NotarizationInfo> {
    println!("\nA keychain profile stores your Apple ID, Team ID and app-specific");
    println!("password in the login keychain under a name you choose. build-info");
    println!("records only that name.\n");

    let default_profile = existing
        .as_ref()
        .and_then(|n| n.keychain_profile.clone())
        .unwrap_or_else(|| "AC_PASSWORD".to_string());

    let profile = Text::new("Keychain profile name:")
        .with_help_message("The label you passed, or will pass, to notarytool store-credentials")
        .with_default(&default_profile)
        .prompt()?;

    if external::notarytool_profile_exists(&profile) {
        println!("Found existing keychain profile \"{}\".", profile);
    } else {
        println!("\nNo keychain profile named \"{}\" was found.", profile);

        let create = Confirm::new("Create it now?")
            .with_help_message("Runs 'xcrun notarytool store-credentials'; notarytool asks for the password itself")
            .with_default(true)
            .prompt()?;

        if create {
            store_credentials_interactive(&profile, existing)?;
        } else {
            println!("\nCreate it later with:\n");
            println!("  xcrun notarytool store-credentials \"{}\" \\", profile);
            println!("      --apple-id \"you@example.com\" \\");
            println!("      --team-id \"YOURTEAMID\" \\");
            println!("      --password \"abcd-efgh-ijkl-mnop\"\n");
        }
    }

    Ok(NotarizationInfo {
        keychain_profile: Some(profile),
        ..Default::default()
    })
}

/// Run `notarytool store-credentials` with inherited stdio.
///
/// The app-specific password is typed straight into notarytool's own prompt —
/// munkipkg never reads, stores, or logs it.
fn store_credentials_interactive(profile: &str, existing: &Option<NotarizationInfo>) -> Result<()> {
    let default_apple_id = existing
        .as_ref()
        .and_then(|n| n.apple_id.clone())
        .unwrap_or_default();
    let default_team_id = existing
        .as_ref()
        .and_then(|n| n.team_id.clone())
        .unwrap_or_default();

    let apple_id = Text::new("Apple ID:")
        .with_help_message("The Apple ID email for your Developer account")
        .with_default(&default_apple_id)
        .prompt()?;

    let team_id = Text::new("Team ID:")
        .with_help_message("Ten characters, e.g. ABC123DEF4")
        .with_default(&default_team_id)
        .prompt()?;

    println!("\nnotarytool will now prompt for your app-specific password.");
    println!("Create one at https://appleid.apple.com under App-Specific Passwords.\n");

    external::notarytool_store_credentials(profile, &apple_id, &team_id)?;
    println!("\nStored keychain profile \"{}\".", profile);

    Ok(())
}

/// Configure Apple ID authentication, with secret-reference support.
fn configure_apple_id(existing: &Option<NotarizationInfo>) -> Result<NotarizationInfo> {
    println!("\nEnter your Apple Developer account details.");
    println!("Supports: op://... (1Password), @env:VAR, @keychain:ITEM\n");
    println!("Tip: Create a .env file in your project with credentials.\n");

    let default_apple_id = existing
        .as_ref()
        .and_then(|n| n.apple_id.clone())
        .unwrap_or_default();

    let default_team_id = existing
        .as_ref()
        .and_then(|n| n.team_id.clone())
        .unwrap_or_default();

    let default_password = existing
        .as_ref()
        .and_then(|n| n.password.clone())
        .unwrap_or_default();

    let apple_id = Text::new("Apple ID:")
        .with_help_message("email, op://vault/item/field, or @env:APPLE_ID")
        .with_default(&default_apple_id)
        .prompt()?;

    let team_id = Text::new("Team ID:")
        .with_help_message("ABC123DEF4, op://..., or @env:TEAM_ID")
        .with_default(&default_team_id)
        .prompt()?;

    println!("\nPassword options:");
    println!("  op://vault/item/password  - 1Password");
    println!("  @keychain:AC_PASSWORD     - macOS Keychain");
    println!("  @env:NOTARIZE_PASSWORD    - Environment / .env file\n");

    let password = Text::new("App-specific password:")
        .with_help_message("op://..., @keychain:..., or @env:...")
        .with_default(&default_password)
        .prompt()?;

    Ok(NotarizationInfo {
        apple_id: Some(apple_id),
        password: Some(password),
        team_id: Some(team_id),
        ..Default::default()
    })
}

/// Configure App Store Connect API key authentication.
fn configure_api_key(existing: &Option<NotarizationInfo>) -> Result<NotarizationInfo> {
    println!("\nEnter your App Store Connect API key details.\n");

    let key_path = Text::new("API key file (.p8) path:")
        .with_help_message("e.g. ~/private_keys/AuthKey_ABC123DEF4.p8")
        .with_default(
            &existing
                .as_ref()
                .and_then(|n| n.api_key_path.clone())
                .unwrap_or_default(),
        )
        .prompt()?;

    let key_id = Text::new("Key ID:")
        .with_default(
            &existing
                .as_ref()
                .and_then(|n| n.api_key_id.clone())
                .unwrap_or_default(),
        )
        .prompt()?;

    let issuer_id = Text::new("Issuer ID:")
        .with_default(
            &existing
                .as_ref()
                .and_then(|n| n.api_issuer_id.clone())
                .unwrap_or_default(),
        )
        .prompt()?;

    Ok(NotarizationInfo {
        api_key_path: Some(key_path),
        api_key_id: Some(key_id),
        api_issuer_id: Some(issuer_id),
        ..Default::default()
    })
}

/// Detect the format of the existing build-info file
fn detect_format(project_dir: &Path) -> Result<OutputFormat> {
    if project_dir.join("build-info.toml").exists() {
        Ok(OutputFormat::Toml)
    } else if project_dir.join("build-info.plist").exists() {
        Ok(OutputFormat::Plist)
    } else if project_dir.join("build-info.json").exists() {
        Ok(OutputFormat::Json)
    } else if project_dir.join("build-info.yaml").exists() {
        Ok(OutputFormat::Yaml)
    } else {
        bail!("No build-info file found in project")
    }
}

/// Quick setup - just add signing identity without prompts
#[allow(dead_code)]
pub fn set_signing_identity(project_dir: &Path, identity: &str) -> Result<()> {
    let mut build_info = BuildInfo::load(project_dir)?;

    // Try to derive installer identity from application identity
    let installer_identity = if identity.contains("Application") {
        Some(identity.replace("Application", "Installer"))
    } else {
        None
    };

    build_info.signing_info = Some(SigningInfo {
        identity: identity.to_string(),
        installer_identity,
        keychain: None,
        additional_cert_names: None,
        timestamp: true,
    });

    let format = detect_format(project_dir)?;
    build_info.save(project_dir, format)?;

    println!("Signing identity set: {}", identity);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_scan_identities() {
        // Just verify it doesn't panic - may or may not find certs
        let _ = scan_application_identities();
        let _ = scan_installer_identities();
    }
}
