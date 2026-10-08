//! Build attestation written beside the package as `<pkg>.provenance.json`.
//!
//! Records what was built, from which inputs, at which commit — enough for a
//! downstream consumer to tie a shipped package back to a tree state.

use crate::clock::Timestamp;
use crate::config::BuildInfo;
use crate::external::GIT;
use crate::hash::{sha256_file, to_hex};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

/// build-info filenames considered part of the build inputs.
const BUILD_INFO_NAMES: [&str; 4] = [
    "build-info.plist",
    "build-info.json",
    "build-info.yaml",
    "build-info.toml",
];

/// A supply-chain attestation for one built package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub tool: String,
    pub tool_version: String,
    pub built_at: String,
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub pkg_path: String,
    pub sha256: String,
    pub input_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_remote: Option<String>,
}

impl Provenance {
    /// Assemble an attestation for `pkg_path`, built from `project_dir`.
    pub fn build(build_info: &BuildInfo, pkg_path: &Path, project_dir: &Path) -> Result<Self> {
        Ok(Self {
            tool: "munkipkg".to_string(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            built_at: Timestamp::now_utc().iso8601_utc(),
            name: build_info.resolved_name(),
            version: build_info.version.clone(),
            identifier: build_info.identifier.clone(),
            pkg_path: pkg_path.display().to_string(),
            sha256: sha256_file(pkg_path)?,
            input_digest: input_digest(project_dir)?,
            git_commit: git_output(project_dir, &["rev-parse", "HEAD"]),
            git_remote: git_output(project_dir, &["remote", "get-url", "origin"])
                .map(|remote| sanitize_remote(&remote)),
        })
    }

    /// Pretty-printed JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// The sidecar path for a package: `mypackage-1.0.pkg.provenance.json`.
    pub fn sidecar_path(pkg_path: &Path) -> PathBuf {
        let mut name = pkg_path.file_name().unwrap_or_default().to_os_string();
        name.push(".provenance.json");
        pkg_path.with_file_name(name)
    }

    /// Write the attestation beside the package, returning the sidecar path.
    pub fn write_sidecar(&self, pkg_path: &Path) -> Result<PathBuf> {
        let path = Self::sidecar_path(pkg_path);
        fs::write(&path, format!("{}\n", self.to_json()?))
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(path)
    }
}

/// Deterministic digest of the build inputs.
///
/// Hashes each input file's project-relative path, permission bits, and
/// contents in sorted order, so the digest is stable across machines and
/// independent of filesystem enumeration order. Symlinks contribute their
/// target rather than the content they point at.
pub fn input_digest(project_dir: &Path) -> Result<String> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    for logical in ["payload", "scripts"] {
        let directory = project_dir.join(logical);
        if !directory.is_dir() {
            continue;
        }

        for entry in WalkDir::new(&directory).into_iter().filter_map(|e| e.ok()) {
            let file_type = entry.file_type();
            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&directory)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| entry.file_name().to_string_lossy().to_string());

            entries.push((
                format!("{}/{}", logical, relative),
                entry.path().to_path_buf(),
            ));
        }
    }

    for name in BUILD_INFO_NAMES {
        let path = project_dir.join(name);
        if path.is_file() {
            entries.push((name.to_string(), path));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (logical_path, path) in entries {
        hasher.update(logical_path.as_bytes());
        hasher.update([0u8]);

        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Failed to stat {}", path.display()))?;
        let mode = (metadata.permissions().mode() & 0o7777) as u16;
        hasher.update(mode.to_le_bytes());

        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)
                .with_context(|| format!("Failed to read link {}", path.display()))?;
            hasher.update(target.to_string_lossy().as_bytes());
        } else {
            let contents =
                fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
            hasher.update(&contents);
        }
    }

    Ok(to_hex(&hasher.finalize()))
}

/// Run git in the project and return trimmed stdout, or `None` if it fails.
fn git_output(project_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(project_dir)
        .args(args)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Strip `user:pass@` userinfo from a remote URL before recording it, so a
/// token embedded in the remote never lands in the attestation.
pub fn sanitize_remote(remote: &str) -> String {
    let Some(scheme_end) = remote.find("://") else {
        return remote.to_string();
    };
    let authority_start = scheme_end + 3;
    let rest = &remote[authority_start..];

    let Some(at) = rest.find('@') else {
        return remote.to_string();
    };
    let first_slash = rest.find('/').unwrap_or(rest.len());
    if at > first_slash {
        return remote.to_string();
    }

    format!("{}{}", &remote[..authority_start], &rest[at + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn project_with(files: &[(&str, &str)]) -> TempDir {
        let temp = TempDir::new().unwrap();
        for (path, contents) in files {
            let full = temp.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, contents).unwrap();
        }
        temp
    }

    #[test]
    fn test_digest_is_stable_across_runs() {
        let project = project_with(&[
            ("payload/Applications/App.txt", "hello"),
            ("build-info.toml", "identifier = \"com.example.app\"\n"),
        ]);

        let first = input_digest(project.path()).unwrap();
        let second = input_digest(project.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn test_digest_changes_when_content_changes() {
        let project = project_with(&[("payload/file.txt", "one")]);
        let before = input_digest(project.path()).unwrap();

        fs::write(project.path().join("payload/file.txt"), "two").unwrap();
        let after = input_digest(project.path()).unwrap();

        assert_ne!(before, after);
    }

    #[test]
    fn test_digest_changes_when_mode_changes() {
        let project = project_with(&[("scripts/postinstall", "#!/bin/sh\n")]);
        let script = project.path().join("scripts/postinstall");
        let before = input_digest(project.path()).unwrap();

        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        let after = input_digest(project.path()).unwrap();

        assert_ne!(before, after, "permission bits must be part of the digest");
    }

    #[test]
    fn test_digest_changes_when_a_file_is_renamed() {
        let project = project_with(&[("payload/a.txt", "same")]);
        let before = input_digest(project.path()).unwrap();

        fs::rename(
            project.path().join("payload/a.txt"),
            project.path().join("payload/b.txt"),
        )
        .unwrap();
        let after = input_digest(project.path()).unwrap();

        assert_ne!(before, after, "logical path must be part of the digest");
    }

    #[test]
    fn test_digest_ignores_the_build_directory() {
        let project = project_with(&[("payload/file.txt", "x")]);
        let before = input_digest(project.path()).unwrap();

        fs::create_dir_all(project.path().join("build")).unwrap();
        fs::write(project.path().join("build/out.pkg"), "artifact").unwrap();
        let after = input_digest(project.path()).unwrap();

        assert_eq!(before, after, "build output is not a build input");
    }

    #[test]
    fn test_digest_records_symlink_target_not_content() {
        let project = project_with(&[("payload/real.txt", "content")]);
        symlink("real.txt", project.path().join("payload/link.txt")).unwrap();
        let before = input_digest(project.path()).unwrap();

        fs::remove_file(project.path().join("payload/link.txt")).unwrap();
        symlink("elsewhere.txt", project.path().join("payload/link.txt")).unwrap();
        let after = input_digest(project.path()).unwrap();

        assert_ne!(before, after);
    }

    #[test]
    fn test_digest_of_empty_project_is_defined() {
        let temp = TempDir::new().unwrap();
        assert_eq!(input_digest(temp.path()).unwrap().len(), 64);
    }

    #[test]
    fn test_sidecar_path_appends_to_the_full_filename() {
        assert_eq!(
            Provenance::sidecar_path(Path::new("/tmp/build/mypackage-1.0.pkg")),
            PathBuf::from("/tmp/build/mypackage-1.0.pkg.provenance.json")
        );
    }

    #[test]
    fn test_sanitize_remote_strips_credentials() {
        assert_eq!(
            sanitize_remote("https://user:token@github.com/acme/repo.git"),
            "https://github.com/acme/repo.git"
        );
    }

    #[test]
    fn test_sanitize_remote_leaves_clean_urls_alone() {
        assert_eq!(
            sanitize_remote("https://github.com/acme/repo.git"),
            "https://github.com/acme/repo.git"
        );
        assert_eq!(
            sanitize_remote("git@github.com:acme/repo.git"),
            "git@github.com:acme/repo.git"
        );
    }

    /// An `@` after the first `/` belongs to the path, not the authority.
    #[test]
    fn test_sanitize_remote_ignores_at_sign_in_path() {
        assert_eq!(
            sanitize_remote("https://example.com/repos/a@b.git"),
            "https://example.com/repos/a@b.git"
        );
    }

    #[test]
    fn test_provenance_json_has_the_documented_keys() {
        let project = project_with(&[("payload/file.txt", "x")]);
        let pkg = project.path().join("out.pkg");
        fs::write(&pkg, b"package bytes").unwrap();

        let build_info = BuildInfo {
            name: "out-${version}.pkg".to_string(),
            identifier: "com.example.out".to_string(),
            version: "3.2".to_string(),
            ..Default::default()
        };

        let provenance = Provenance::build(&build_info, &pkg, project.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&provenance.to_json().unwrap()).unwrap();

        assert_eq!(json["tool"], "munkipkg");
        assert_eq!(json["identifier"], "com.example.out");
        assert_eq!(json["version"], "3.2");
        assert_eq!(json["name"], "out-3.2.pkg");
        assert!(json["built_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(json["input_digest"].as_str().unwrap().len(), 64);
        assert_eq!(json["sha256"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn test_write_sidecar_lands_next_to_the_package() {
        let project = project_with(&[("payload/file.txt", "x")]);
        let pkg = project.path().join("out.pkg");
        fs::write(&pkg, b"bytes").unwrap();

        let build_info = BuildInfo {
            name: "out.pkg".to_string(),
            identifier: "com.example.out".to_string(),
            ..Default::default()
        };

        let provenance = Provenance::build(&build_info, &pkg, project.path()).unwrap();
        let written = provenance.write_sidecar(&pkg).unwrap();

        assert!(written.exists());
        assert_eq!(written, project.path().join("out.pkg.provenance.json"));
    }
}
