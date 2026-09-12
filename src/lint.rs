//! Project validation without building — a fast pre-check for PR CI.
//!
//! `munkipkg lint` loads build-info, checks the project for the mistakes that
//! only surface minutes into a signed build, and exits non-zero on any error.

use crate::config::BuildInfo;
use crate::errors::invalid_config;
use crate::project::{has_payload, has_scripts};
use anyhow::Result;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The project should not build.
    Error,
    /// Advisory; the build will proceed.
    Warning,
    /// Informational only. Never fails a build, not even under `--strict`.
    Note,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Note => write!(f, "note"),
        }
    }
}

/// One problem found by the linter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    pub fn note(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Note,
            message: message.into(),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity, self.message)
    }
}

/// Validate a project directory. Returns every finding.
///
/// Only a genuinely unreadable project — a missing directory — is an `Err`;
/// an undecodable build-info becomes an error finding so `lint` always produces
/// a report rather than a stack trace.
pub fn lint(project_dir: &Path) -> Result<Vec<Finding>> {
    if !project_dir.is_dir() {
        return Err(
            invalid_config(format!("{}: Project not found.", project_dir.display())).into(),
        );
    }

    let build_info = match BuildInfo::load(project_dir) {
        Ok(info) => info,
        Err(error) => return Ok(vec![Finding::error(format!("{:#}", error))]),
    };

    let mut findings = Vec::new();

    if build_info.identifier.is_empty() {
        findings.push(Finding::error("identifier is empty"));
    } else if !is_reverse_dns(&build_info.identifier) {
        findings.push(Finding::warning(format!(
            "identifier \"{}\" is not reverse-DNS style",
            build_info.identifier
        )));
    }

    if build_info.version.is_empty() {
        findings.push(Finding::error("version is empty"));
    }

    let name = &build_info.name;
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        findings.push(Finding::error(format!(
            "name \"{}\" must be a single path component",
            name
        )));
    }

    findings.extend(lint_signing(&build_info));
    findings.extend(lint_app_version(&build_info, project_dir));

    // Mirror the build's own rule: a project with no payload content and no
    // install script has nothing to package, and `build` refuses it. Catching
    // that here is the whole point of a pre-check.
    if !has_payload(project_dir) && !has_scripts(project_dir) {
        findings.push(Finding::error(
            "project has no payload and no scripts. Nothing to build.",
        ));
    }

    let scripts = project_dir.join("scripts");
    if has_meaningful_contents(&scripts) {
        findings.extend(lint_scripts(&scripts));
    }

    // Surface configuration errors the build would reject anyway, so `lint`
    // catches them in CI before any signing identity is touched.
    if let Err(error) = build_info.validate() {
        findings.push(Finding::error(format!("{:#}", error)));
    }

    Ok(findings)
}

/// True when any finding would stop a build.
pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

/// Reverse-DNS means at least two dot-separated, non-empty components, so
/// leading, repeated, and trailing dots (`.a`, `a..b`, `a.b.`) are rejected.
pub fn is_reverse_dns(identifier: &str) -> bool {
    let parts: Vec<&str> = identifier.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty())
}

/// When `version` asks for an application's own version, confirm the payload
/// can actually answer. Otherwise the failure only appears at build time.
fn lint_app_version(build_info: &BuildInfo, project_dir: &Path) -> Vec<Finding> {
    if !crate::dynamic_version::contains_app_token(&build_info.version) {
        return Vec::new();
    }

    let app = match crate::appinfo::find_primary(&project_dir.join("payload")) {
        Ok(app) => app,
        Err(error) => return vec![Finding::error(format!("{:#}", error))],
    };

    match crate::dynamic_version::resolve_app(&build_info.version, &app) {
        Ok(resolved) => vec![Finding::note(format!(
            "version resolves to \"{}\" from {}",
            resolved, app.relative_path
        ))],
        Err(error) => vec![Finding::error(format!("{:#}", error))],
    }
}

fn lint_signing(build_info: &BuildInfo) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(notarization) = &build_info.notarization_info else {
        return findings;
    };

    if build_info.signing_info.is_none() {
        findings.push(Finding::warning(
            "notarization is configured but signing is not; notarization requires a Developer ID signature",
        ));
    }

    if !notarization.has_credentials() {
        findings.push(Finding::error(
            "notarization_info has no credentials: set keychain_profile, or apple_id with team_id and password, or an API key",
        ));
    }

    findings
}

fn lint_scripts(scripts: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();

    for name in ["preinstall", "postinstall"] {
        let script = scripts.join(name);
        let Ok(metadata) = fs::symlink_metadata(&script) else {
            continue;
        };

        if metadata.is_dir() {
            findings.push(Finding::error(format!(
                "{} is a directory, but an install script must be a regular file",
                name
            )));
            continue;
        }

        if let Ok(contents) = fs::read(&script)
            && !contents.starts_with(b"#!")
        {
            findings.push(Finding::warning(format!(
                "{} script does not start with a shebang (#!)",
                name
            )));
        }

        if metadata.permissions().mode() & 0o111 == 0 {
            findings.push(Finding::warning(format!(
                "{} script is not executable",
                name
            )));
        }
    }

    findings
}

/// A directory with nothing but `.DS_Store` counts as empty.
fn has_meaningful_contents(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .any(|e| e.file_name() != ".DS_Store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn project(build_info: &str) -> TempDir {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("build-info.toml"), build_info).unwrap();
        fs::create_dir_all(temp.path().join("payload")).unwrap();
        fs::write(temp.path().join("payload/placeholder"), "x").unwrap();
        temp
    }

    fn valid() -> &'static str {
        "name = \"app-${version}.pkg\"\nidentifier = \"com.example.app\"\nversion = \"1.0\"\n"
    }

    fn messages(findings: &[Finding]) -> String {
        findings
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_clean_project_has_no_findings() {
        let temp = project(valid());
        let findings = lint(temp.path()).unwrap();
        assert!(
            findings.is_empty(),
            "unexpected findings: {}",
            messages(&findings)
        );
    }

    #[test]
    fn test_missing_project_is_an_error() {
        let temp = TempDir::new().unwrap();
        assert!(lint(&temp.path().join("nope")).is_err());
    }

    #[test]
    fn test_missing_build_info_is_a_finding_not_a_crash() {
        let temp = TempDir::new().unwrap();
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(findings[0].message.contains("No build-info file found"));
    }

    #[test]
    fn test_unparseable_build_info_is_a_finding() {
        let temp = project("this is not = valid toml [[[");
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
    }

    #[test]
    fn test_empty_identifier_is_an_error() {
        let temp = project("name = \"a.pkg\"\nidentifier = \"\"\nversion = \"1.0\"\n");
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(messages(&findings).contains("identifier is empty"));
    }

    #[test]
    fn test_non_reverse_dns_identifier_is_a_warning() {
        let temp = project("name = \"a.pkg\"\nidentifier = \"myapp\"\nversion = \"1.0\"\n");
        let findings = lint(temp.path()).unwrap();
        assert!(!has_errors(&findings));
        assert!(messages(&findings).contains("not reverse-DNS"));
    }

    #[test]
    fn test_reverse_dns_rules() {
        assert!(is_reverse_dns("com.example.app"));
        assert!(is_reverse_dns("com.example"));
        assert!(!is_reverse_dns("app"));
        assert!(!is_reverse_dns(".com.example"));
        assert!(!is_reverse_dns("com..example"));
        assert!(!is_reverse_dns("com.example."));
        assert!(!is_reverse_dns(""));
    }

    #[test]
    fn test_name_with_a_slash_is_an_error() {
        let temp =
            project("name = \"build/a.pkg\"\nidentifier = \"com.example.a\"\nversion = \"1.0\"\n");
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(messages(&findings).contains("single path component"));
    }

    #[test]
    fn test_notarization_without_signing_is_a_warning() {
        let temp = project(
            "name = \"a.pkg\"\nidentifier = \"com.example.a\"\nversion = \"1.0\"\n\
             [notarization_info]\nkeychain_profile = \"AC_PASSWORD\"\n",
        );
        let findings = lint(temp.path()).unwrap();
        assert!(!has_errors(&findings));
        assert!(messages(&findings).contains("requires a Developer ID signature"));
    }

    #[test]
    fn test_notarization_without_credentials_is_an_error() {
        let temp = project(
            "name = \"a.pkg\"\nidentifier = \"com.example.a\"\nversion = \"1.0\"\n\
             [signing_info]\nidentity = \"Developer ID Installer: Example (TEAMID)\"\n\
             [notarization_info]\n",
        );
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(messages(&findings).contains("no credentials"));
    }

    #[test]
    fn test_keychain_profile_counts_as_credentials() {
        let temp = project(
            "name = \"a.pkg\"\nidentifier = \"com.example.a\"\nversion = \"1.0\"\n\
             [signing_info]\nidentity = \"Developer ID Installer: Example (TEAMID)\"\n\
             [notarization_info]\nkeychain_profile = \"AC_PASSWORD\"\n",
        );
        let findings = lint(temp.path()).unwrap();
        assert!(findings.is_empty(), "unexpected: {}", messages(&findings));
    }

    #[test]
    fn test_script_without_shebang_is_a_warning() {
        let temp = project(valid());
        let scripts = temp.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join("postinstall"), "echo hi\n").unwrap();

        let findings = lint(temp.path()).unwrap();
        assert!(!has_errors(&findings));
        assert!(messages(&findings).contains("shebang"));
    }

    #[test]
    fn test_non_executable_script_is_a_warning() {
        let temp = project(valid());
        let scripts = temp.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        let script = scripts.join("preinstall");
        fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&script, perms).unwrap();

        let findings = lint(temp.path()).unwrap();
        assert!(messages(&findings).contains("not executable"));
    }

    #[test]
    fn test_executable_script_with_shebang_is_clean() {
        let temp = project(valid());
        let scripts = temp.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        let script = scripts.join("postinstall");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let findings = lint(temp.path()).unwrap();
        assert!(findings.is_empty(), "unexpected: {}", messages(&findings));
    }

    #[test]
    fn test_script_directory_is_an_error() {
        let temp = project(valid());
        fs::create_dir_all(temp.path().join("scripts/postinstall")).unwrap();

        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(messages(&findings).contains("must be a regular file"));
    }

    #[test]
    fn test_scripts_dir_with_only_ds_store_is_ignored() {
        let temp = project(valid());
        let scripts = temp.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join(".DS_Store"), "junk").unwrap();

        let findings = lint(temp.path()).unwrap();
        assert!(findings.is_empty(), "unexpected: {}", messages(&findings));
    }

    #[test]
    fn test_large_payload_without_min_os_is_an_error() {
        let temp = project(
            "name = \"a.pkg\"\nidentifier = \"com.example.a\"\nversion = \"1.0\"\n\
             large-payload = true\n",
        );
        let findings = lint(temp.path()).unwrap();
        assert!(has_errors(&findings));
        assert!(messages(&findings).contains("large-payload"));
    }
}
