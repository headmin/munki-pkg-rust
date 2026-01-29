//! Configuration handling for munki-pkg projects
//!
//! Supports multiple formats: plist (XML), JSON, YAML, and TOML.
//! Original format support by Greg Neagle's munki-pkg.
//! TOML support added in this Rust port.

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Output format for build-info files
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Plist,
    Json,
    Yaml,
    Toml,
}

impl OutputFormat {
    pub fn filename(&self) -> &'static str {
        match self {
            OutputFormat::Plist => "build-info.plist",
            OutputFormat::Json => "build-info.json",
            OutputFormat::Yaml => "build-info.yaml",
            OutputFormat::Toml => "build-info.toml",
        }
    }
}

/// File ownership options for pkgbuild
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Ownership {
    #[default]
    Recommended,
    Preserve,
    #[serde(rename = "preserve-other")]
    PreserveOther,
}

impl Ownership {
    pub fn as_str(&self) -> &'static str {
        match self {
            Ownership::Recommended => "recommended",
            Ownership::Preserve => "preserve",
            Ownership::PreserveOther => "preserve-other",
        }
    }
}

/// Post-install action options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PostinstallAction {
    #[default]
    None,
    Logout,
    Restart,
}

impl PostinstallAction {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            PostinstallAction::None => "none",
            PostinstallAction::Logout => "logout",
            PostinstallAction::Restart => "restart",
        }
    }
}

/// Compression options for pkgbuild
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Legacy,
    Latest,
}

impl Compression {
    pub fn as_str(&self) -> &'static str {
        match self {
            Compression::Legacy => "legacy",
            Compression::Latest => "latest",
        }
    }
}

/// Signing configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigningInfo {
    /// Signing identity (certificate common name)
    pub identity: String,

    /// Path to keychain containing the signing certificate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keychain: Option<String>,

    /// Additional certificate names to include
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_cert_names: Option<Vec<String>>,

    /// Include a secure timestamp
    #[serde(default = "default_true")]
    pub timestamp: bool,
}

/// Notarization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotarizationInfo {
    /// Apple ID for notarization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apple_id: Option<String>,

    /// Password (use @keychain:ITEM_NAME for keychain lookup)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Team ID for App Store Connect
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// App Store Connect provider (for accounts in multiple teams)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asc_provider: Option<String>,

    /// Primary bundle ID for the submission
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_bundle_id: Option<String>,

    /// Timeout in seconds for stapling (default 300)
    #[serde(default = "default_staple_timeout")]
    pub staple_timeout: u32,

    /// Path to App Store Connect API key file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_path: Option<String>,

    /// App Store Connect API key ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,

    /// App Store Connect API issuer ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_issuer_id: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_staple_timeout() -> u32 {
    300
}

/// Main build configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Package name (can include ${version} placeholder)
    #[serde(default)]
    pub name: String,

    /// Package bundle identifier
    #[serde(default)]
    pub identifier: String,

    /// Package version string
    #[serde(default = "default_version")]
    pub version: String,

    /// File ownership handling
    #[serde(default)]
    pub ownership: Ownership,

    /// Installation location on target system
    #[serde(default = "default_install_location")]
    pub install_location: String,

    /// Build as distribution-style package
    #[serde(default)]
    pub distribution_style: bool,

    /// Suppress bundle relocation
    #[serde(default = "default_true")]
    pub suppress_bundle_relocation: bool,

    /// Action after installation completes
    #[serde(default)]
    pub postinstall_action: PostinstallAction,

    /// Preserve extended attributes (code signatures, etc.)
    #[serde(default)]
    pub preserve_xattr: bool,

    /// Compression method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<Compression>,

    /// Minimum OS version required
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "min-os-version")]
    pub min_os_version: Option<String>,

    /// Use large payload support (macOS 12+)
    #[serde(default)]
    #[serde(rename = "large-payload")]
    pub large_payload: bool,

    /// Product ID for distribution packages
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "product id")]
    pub product_id: Option<String>,

    /// Signing configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_info: Option<SigningInfo>,

    /// Notarization configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notarization_info: Option<NotarizationInfo>,

    /// Additional pkgbuild options (for advanced users)
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub additional_options: HashMap<String, String>,
}

fn default_version() -> String {
    "1.0".to_string()
}

fn default_install_location() -> String {
    "/".to_string()
}

impl Default for BuildInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            identifier: String::new(),
            version: default_version(),
            ownership: Ownership::default(),
            install_location: default_install_location(),
            distribution_style: false,
            suppress_bundle_relocation: true,
            postinstall_action: PostinstallAction::default(),
            preserve_xattr: false,
            compression: None,
            min_os_version: None,
            large_payload: false,
            product_id: None,
            signing_info: None,
            notarization_info: None,
            additional_options: HashMap::new(),
        }
    }
}

impl BuildInfo {
    /// Create BuildInfo with defaults for a new project
    pub fn new_project(project_name: &str) -> Self {
        Self {
            name: format!("{}-${{version}}.pkg", project_name),
            identifier: format!("com.github.munki.pkg.{}", project_name),
            ..Default::default()
        }
    }

    /// Get the resolved package name (with version substituted)
    pub fn resolved_name(&self) -> String {
        self.name.replace("${version}", &self.version)
    }

    /// Load build info from a project directory
    /// Tries formats in order: plist > json > yaml > toml
    pub fn load(project_dir: &Path) -> Result<Self> {
        // Try each format in priority order
        let formats = [
            ("build-info.plist", "plist"),
            ("build-info.json", "json"),
            ("build-info.yaml", "yaml"),
            ("build-info.toml", "toml"),
        ];

        for (filename, format) in formats {
            let path = project_dir.join(filename);
            if path.exists() {
                return Self::load_file(&path, format);
            }
        }

        bail!(
            "No build-info file found in {}. Expected one of: build-info.plist, build-info.json, build-info.yaml, or build-info.toml",
            project_dir.display()
        );
    }

    /// Load from a specific file
    fn load_file(path: &Path, format: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let info: BuildInfo = match format {
            "plist" => {
                // For plist, read as bytes since it could be binary
                let bytes =
                    fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
                plist::from_bytes(&bytes)
                    .with_context(|| format!("Failed to parse plist: {}", path.display()))?
            }
            "json" => serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON: {}", path.display()))?,
            "yaml" => yaml_serde::from_str(&content)
                .with_context(|| format!("Failed to parse YAML: {}", path.display()))?,
            "toml" => toml::from_str(&content)
                .with_context(|| format!("Failed to parse TOML: {}", path.display()))?,
            _ => bail!("Unknown format: {}", format),
        };

        Ok(info)
    }

    /// Save build info to a file in the specified format
    pub fn save(&self, project_dir: &Path, format: OutputFormat) -> Result<()> {
        let filename = format.filename();
        let path = project_dir.join(filename);

        let content = match format {
            OutputFormat::Plist => {
                let mut buf = Vec::new();
                plist::to_writer_xml(&mut buf, self).context("Failed to serialize to plist")?;
                fs::write(&path, buf)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                return Ok(());
            }
            OutputFormat::Json => {
                serde_json::to_string_pretty(self).context("Failed to serialize to JSON")?
            }
            OutputFormat::Yaml => {
                yaml_serde::to_string(self).context("Failed to serialize to YAML")?
            }
            OutputFormat::Toml => {
                toml::to_string_pretty(self).context("Failed to serialize to TOML")?
            }
        };

        fs::write(&path, content).with_context(|| format!("Failed to write {}", path.display()))?;

        Ok(())
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        if self.identifier.is_empty() {
            bail!("Package identifier is required");
        }

        if self.large_payload {
            if let Some(ref min_os) = self.min_os_version {
                let parts: Vec<&str> = min_os.split('.').collect();
                if let Some(major) = parts.first()
                    && let Ok(major_num) = major.parse::<u32>()
                    && major_num < 12
                {
                    bail!("large-payload requires min-os-version >= 12.0");
                }
            } else {
                bail!("large-payload requires min-os-version to be set (>= 12.0)");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_build_info() {
        let info = BuildInfo::default();
        assert_eq!(info.version, "1.0");
        assert_eq!(info.install_location, "/");
        assert!(info.suppress_bundle_relocation);
        assert_eq!(info.ownership, Ownership::Recommended);
    }

    #[test]
    fn test_new_project() {
        let info = BuildInfo::new_project("MyApp");
        assert_eq!(info.name, "MyApp-${version}.pkg");
        assert_eq!(info.identifier, "com.github.munki.pkg.MyApp");
    }

    #[test]
    fn test_resolved_name() {
        let mut info = BuildInfo::new_project("MyApp");
        info.version = "2.5".to_string();
        assert_eq!(info.resolved_name(), "MyApp-2.5.pkg");
    }

    #[test]
    fn test_ownership_as_str() {
        assert_eq!(Ownership::Recommended.as_str(), "recommended");
        assert_eq!(Ownership::Preserve.as_str(), "preserve");
        assert_eq!(Ownership::PreserveOther.as_str(), "preserve-other");
    }
}
