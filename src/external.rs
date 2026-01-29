//! External command execution for macOS tools
//!
//! Wraps calls to pkgbuild, productbuild, pkgutil, and other
//! macOS command-line tools used for package building.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Command, Output};

/// Path to macOS ditto command
pub const DITTO: &str = "/usr/bin/ditto";

/// Path to macOS lsbom command (list bill of materials)
pub const LSBOM: &str = "/usr/bin/lsbom";

/// Path to macOS pkgbuild command
pub const PKGBUILD: &str = "/usr/bin/pkgbuild";

/// Path to macOS pkgutil command
pub const PKGUTIL: &str = "/usr/sbin/pkgutil";

/// Path to macOS productbuild command
pub const PRODUCTBUILD: &str = "/usr/bin/productbuild";

/// Path to macOS xcrun command
pub const XCRUN: &str = "/usr/bin/xcrun";

/// Path to macOS productsign command
pub const PRODUCTSIGN: &str = "/usr/bin/productsign";

/// Execute a command and return the output
pub fn run_command(cmd: &mut Command) -> Result<Output> {
    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute: {:?}", cmd))?;

    Ok(output)
}

/// Execute a command and require success
pub fn run_command_checked(cmd: &mut Command) -> Result<Output> {
    let output = run_command(cmd)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "Command failed with exit code {:?}:\n{}\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(output)
}

/// Check if a required tool exists
pub fn check_tool(path: &str) -> Result<()> {
    if !Path::new(path).exists() {
        bail!(
            "Required tool not found: {}. This tool requires macOS.",
            path
        );
    }
    Ok(())
}

/// Verify all required tools are available
pub fn verify_tools() -> Result<()> {
    let tools = [PKGBUILD, PRODUCTBUILD, PKGUTIL, LSBOM, DITTO];
    for tool in tools {
        check_tool(tool)?;
    }
    Ok(())
}

/// Run pkgbuild --analyze to create a component property list
pub fn pkgbuild_analyze(root_dir: &Path, output_plist: &Path) -> Result<()> {
    let mut cmd = Command::new(PKGBUILD);
    cmd.arg("--analyze")
        .arg("--root")
        .arg(root_dir)
        .arg(output_plist);

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Expand a package using pkgutil
pub fn pkgutil_expand(pkg_path: &Path, dest_dir: &Path) -> Result<()> {
    let mut cmd = Command::new(PKGUTIL);
    cmd.arg("--expand").arg(pkg_path).arg(dest_dir);

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Extract BOM info using lsbom
pub fn lsbom_extract(bom_path: &Path) -> Result<String> {
    let mut cmd = Command::new(LSBOM);
    cmd.arg("-s").arg("-f").arg("-l").arg("-m").arg(bom_path);

    let output = run_command_checked(&mut cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Extract archive using ditto
pub fn ditto_extract(archive: &Path, dest: &Path) -> Result<()> {
    let mut cmd = Command::new(DITTO);
    cmd.arg("-x").arg(archive).arg(dest);

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Copy directory using ditto (preserves metadata)
pub fn ditto_copy(src: &Path, dest: &Path) -> Result<()> {
    let mut cmd = Command::new(DITTO);
    cmd.arg(src).arg(dest);

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Submit package for notarization using notarytool
pub fn notarytool_submit(
    pkg_path: &Path,
    apple_id: Option<&str>,
    password: Option<&str>,
    team_id: Option<&str>,
    api_key_path: Option<&str>,
    api_key_id: Option<&str>,
    api_issuer_id: Option<&str>,
) -> Result<String> {
    let mut cmd = Command::new(XCRUN);
    cmd.arg("notarytool").arg("submit").arg(pkg_path);

    if let Some(id) = apple_id {
        cmd.arg("--apple-id").arg(id);
    }
    if let Some(pwd) = password {
        cmd.arg("--password").arg(pwd);
    }
    if let Some(team) = team_id {
        cmd.arg("--team-id").arg(team);
    }
    if let Some(key_path) = api_key_path {
        cmd.arg("--key").arg(key_path);
    }
    if let Some(key_id) = api_key_id {
        cmd.arg("--key-id").arg(key_id);
    }
    if let Some(issuer) = api_issuer_id {
        cmd.arg("--issuer").arg(issuer);
    }

    cmd.arg("--wait").arg("--output-format").arg("json");

    let output = run_command_checked(&mut cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Staple notarization ticket to package
pub fn stapler_staple(pkg_path: &Path) -> Result<()> {
    let mut cmd = Command::new(XCRUN);
    cmd.arg("stapler").arg("staple").arg(pkg_path);

    run_command_checked(&mut cmd)?;
    Ok(())
}

/// Sign a package using productsign
pub fn productsign(
    input_pkg: &Path,
    output_pkg: &Path,
    identity: &str,
    keychain: Option<&str>,
    timestamp: bool,
) -> Result<()> {
    let mut cmd = Command::new(PRODUCTSIGN);
    cmd.arg("--sign").arg(identity);

    if let Some(kc) = keychain {
        cmd.arg("--keychain").arg(kc);
    }

    if timestamp {
        cmd.arg("--timestamp");
    }

    cmd.arg(input_pkg).arg(output_pkg);

    run_command_checked(&mut cmd)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_tools_exist() {
        // This test only runs on macOS with Xcode command line tools
        // We just check that the path constant is valid
        let _ = Path::new(PKGBUILD);
    }
}
