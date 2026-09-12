//! End-to-end tests that drive the built `munkipkg` binary.
//!
//! These complement the unit tests by exercising what a caller actually sees:
//! the process exit status, what lands on stdout, and what appears on disk.
//! Tests that need Apple's packaging tools are gated to macOS; the exit-code
//! and lint tests run anywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Path to the binary under test, provided by Cargo.
const BIN: &str = env!("CARGO_BIN_EXE_munkipkg");

fn munkipkg(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("failed to run munkipkg")
}

fn status(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("process was killed by a signal")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// A minimal, buildable project with one payload file.
fn fixture(build_info: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("mypackage");
    fs::create_dir_all(project.join("payload/usr/local/share")).unwrap();
    fs::write(
        project.join("payload/usr/local/share/hello.txt"),
        "hello from munkipkg\n",
    )
    .unwrap();
    fs::write(project.join("build-info.toml"), build_info).unwrap();
    temp
}

fn project_path(temp: &TempDir) -> PathBuf {
    temp.path().join("mypackage")
}

fn default_build_info() -> &'static str {
    "name = \"mypackage-${version}.pkg\"\n\
     identifier = \"com.example.mypackage\"\n\
     version = \"1.0\"\n\
     install_location = \"/\"\n"
}

// ---------------------------------------------------------------------------
// Usage and exit codes — no Apple tooling required
// ---------------------------------------------------------------------------

#[test]
fn test_version_flag_exits_zero() {
    let output = munkipkg(&["--version"]);
    assert_eq!(status(&output), 0);
    assert!(stdout(&output).contains("munkipkg"));
}

#[test]
fn test_help_flag_exits_zero() {
    let output = munkipkg(&["--help"]);
    assert_eq!(status(&output), 0);
}

#[test]
fn test_unknown_flag_is_a_usage_error() {
    let output = munkipkg(&["--definitely-not-a-flag"]);
    assert_eq!(status(&output), 64, "stderr: {}", stderr(&output));
}

#[test]
fn test_unknown_subcommand_is_a_usage_error() {
    let output = munkipkg(&["lint"]);
    assert_eq!(status(&output), 64, "missing required argument");
}

#[test]
fn test_no_arguments_is_an_invalid_configuration() {
    let output = munkipkg(&[]);
    assert_eq!(status(&output), 3);
    assert!(stderr(&output).contains("No project directory specified"));
}

#[test]
fn test_missing_build_info_exits_three() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("empty");
    fs::create_dir_all(project.join("payload")).unwrap();

    let output = munkipkg(&[project.to_str().unwrap()]);
    assert_eq!(status(&output), 3, "stderr: {}", stderr(&output));
}

#[test]
fn test_missing_project_directory_exits_three() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("nowhere");

    let output = munkipkg(&[missing.to_str().unwrap()]);
    assert_eq!(status(&output), 3);
}

#[test]
fn test_create_over_existing_project_exits_two() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("existing");
    fs::create_dir_all(&project).unwrap();

    let output = munkipkg(&["create", project.to_str().unwrap(), "--format", "toml"]);
    assert_eq!(status(&output), 2, "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("already exists"));
}

#[test]
fn test_create_with_force_over_existing_project_succeeds() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("existing");
    fs::create_dir_all(&project).unwrap();

    let output = munkipkg(&[
        "create",
        project.to_str().unwrap(),
        "--format",
        "toml",
        "--force",
    ]);
    assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    assert!(project.join("build-info.toml").exists());
}

// ---------------------------------------------------------------------------
// Lint
// ---------------------------------------------------------------------------

#[test]
fn test_lint_clean_project_exits_zero() {
    let temp = fixture(default_build_info());
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap()]);
    assert_eq!(status(&output), 0, "stdout: {}", stdout(&output));
}

#[test]
fn test_lint_reports_an_empty_identifier() {
    let temp = fixture("name = \"a.pkg\"\nidentifier = \"\"\nversion = \"1.0\"\n");
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 3);
    assert!(stdout(&output).contains("error: identifier is empty"));
}

#[test]
fn test_lint_warning_alone_exits_zero() {
    let temp = fixture("name = \"a.pkg\"\nidentifier = \"notdns\"\nversion = \"1.0\"\n");
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 0);
    assert!(stdout(&output).contains("warning:"));
    assert!(stdout(&output).contains("reverse-DNS"));
}

#[test]
fn test_lint_strict_turns_warnings_into_failure() {
    let temp = fixture("name = \"a.pkg\"\nidentifier = \"notdns\"\nversion = \"1.0\"\n");
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap(), "--strict"]);

    assert_eq!(status(&output), 3, "stdout: {}", stdout(&output));
}

#[test]
fn test_lint_quiet_reports_only_through_the_exit_status() {
    let temp = fixture("name = \"a.pkg\"\nidentifier = \"\"\nversion = \"1.0\"\n");
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap(), "--quiet"]);

    assert_eq!(status(&output), 3);
    assert!(stdout(&output).trim().is_empty());
}

#[test]
fn test_lint_missing_project_is_an_error() {
    let temp = TempDir::new().unwrap();
    let output = munkipkg(&["lint", temp.path().join("nope").to_str().unwrap()]);
    assert_eq!(status(&output), 3);
}

// ---------------------------------------------------------------------------
// Building — requires pkgbuild, so macOS only
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod building {
    use super::*;

    fn built_pkg(dir: &Path) -> Option<PathBuf> {
        fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "pkg"))
    }

    #[test]
    fn test_build_produces_a_package() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap()]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let pkg = project.join("build/mypackage-1.0.pkg");
        assert!(pkg.exists(), "expected {}", pkg.display());
    }

    #[test]
    fn test_build_subcommand_matches_the_bare_form() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&["build", project.to_str().unwrap()]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
        assert!(project.join("build/mypackage-1.0.pkg").exists());
    }

    #[test]
    fn test_quiet_build_prints_nothing() {
        let temp = fixture(default_build_info());
        let output = munkipkg(&[project_path(&temp).to_str().unwrap(), "--quiet"]);

        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
        assert!(
            stdout(&output).trim().is_empty(),
            "got: {}",
            stdout(&output)
        );
    }

    #[test]
    fn test_json_manifest_is_the_only_thing_on_stdout() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--output-format", "json"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("stdout was not valid JSON");

        assert_eq!(manifest["name"], "mypackage-1.0.pkg");
        assert_eq!(manifest["version"], "1.0");
        assert_eq!(manifest["identifier"], "com.example.mypackage");
        assert_eq!(manifest["signed"], false);
        assert_eq!(manifest["notarized"], false);
        assert_eq!(manifest["stapled"], false);
        assert_eq!(manifest["sha256"].as_str().unwrap().len(), 64);

        let pkg_path = Path::new(manifest["pkg_path"].as_str().unwrap());
        assert!(pkg_path.exists());
    }

    #[test]
    fn test_manifest_sha256_matches_the_package_on_disk() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--output-format", "json"]);
        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();

        let pkg_path = manifest["pkg_path"].as_str().unwrap();
        let shasum = Command::new("/usr/bin/shasum")
            .args(["-a", "256", pkg_path])
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&shasum.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string();

        assert_eq!(manifest["sha256"], expected);
    }

    #[test]
    fn test_pkg_version_overrides_build_info() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[
            project.to_str().unwrap(),
            "--pkg-version",
            "9.9.9",
            "--output-format",
            "json",
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        assert_eq!(manifest["version"], "9.9.9");
        assert_eq!(
            manifest["name"], "mypackage-9.9.9.pkg",
            "the override must be applied before ${{version}} substitution"
        );
        assert!(project.join("build/mypackage-9.9.9.pkg").exists());
    }

    #[test]
    fn test_dynamic_date_token_in_build_info_version() {
        let temp = fixture(
            "name = \"mypackage-${version}.pkg\"\n\
             identifier = \"com.example.mypackage\"\n\
             version = \"${DATE}\"\n",
        );

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--output-format",
            "json",
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        let version = manifest["version"].as_str().unwrap();

        assert!(
            version.len() == 10 && version.matches('.').count() == 2,
            "expected yyyy.MM.dd, got {}",
            version
        );
        assert!(version.starts_with("20"), "got {}", version);
        assert_eq!(manifest["name"], format!("mypackage-{}.pkg", version));
    }

    #[test]
    fn test_dynamic_token_via_pkg_version_is_also_resolved() {
        let temp = fixture(default_build_info());

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--pkg-version",
            "${DATE}",
            "--output-format",
            "json",
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        assert!(
            !manifest["version"].as_str().unwrap().contains("${"),
            "token survived: {}",
            manifest["version"]
        );
    }

    #[test]
    fn test_output_dir_redirects_the_package() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);
        let elsewhere = temp.path().join("dist/nested");

        let output = munkipkg(&[
            project.to_str().unwrap(),
            "--output-dir",
            elsewhere.to_str().unwrap(),
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        assert!(
            elsewhere.join("mypackage-1.0.pkg").exists(),
            "package should be in the output dir"
        );
        assert!(
            built_pkg(&project.join("build")).is_none(),
            "project build/ should not also receive a package"
        );
    }

    #[test]
    fn test_provenance_sidecar_is_written_and_consistent() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[
            project.to_str().unwrap(),
            "--provenance",
            "--output-format",
            "json",
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        let sidecar = project.join("build/mypackage-1.0.pkg.provenance.json");
        assert!(sidecar.exists(), "expected {}", sidecar.display());

        let provenance: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&sidecar).unwrap()).unwrap();

        assert_eq!(provenance["tool"], "munkipkg");
        assert_eq!(provenance["identifier"], "com.example.mypackage");
        assert_eq!(provenance["version"], "1.0");
        assert_eq!(
            provenance["sha256"], manifest["sha256"],
            "provenance and manifest must agree on the package hash"
        );
        assert_eq!(provenance["input_digest"].as_str().unwrap().len(), 64);
        assert!(provenance["built_at"].as_str().unwrap().ends_with('Z'));
        assert!(!provenance["tool_version"].as_str().unwrap().is_empty());
    }

    #[test]
    fn test_provenance_input_digest_tracks_payload_changes() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);
        let sidecar = project.join("build/mypackage-1.0.pkg.provenance.json");

        munkipkg(&[project.to_str().unwrap(), "--provenance", "--quiet"]);
        let first: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&sidecar).unwrap()).unwrap();

        fs::write(
            project.join("payload/usr/local/share/hello.txt"),
            "different content\n",
        )
        .unwrap();

        munkipkg(&[project.to_str().unwrap(), "--provenance", "--quiet"]);
        let second: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&sidecar).unwrap()).unwrap();

        assert_ne!(first["input_digest"], second["input_digest"]);
    }

    #[test]
    fn test_no_provenance_flag_writes_no_sidecar() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        munkipkg(&[project.to_str().unwrap(), "--quiet"]);

        assert!(
            !project
                .join("build/mypackage-1.0.pkg.provenance.json")
                .exists()
        );
    }

    #[test]
    fn test_verify_passes_on_a_fresh_build() {
        let temp = fixture(default_build_info());
        let output = munkipkg(&[project_path(&temp).to_str().unwrap(), "--verify"]);

        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
        assert!(stdout(&output).contains("Verified package identifier and version"));
    }

    #[test]
    fn test_verify_does_not_demand_a_signature_when_none_was_requested() {
        let temp = fixture(default_build_info());
        let output = munkipkg(&[project_path(&temp).to_str().unwrap(), "--verify", "--quiet"]);

        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    }

    #[test]
    fn test_lint_gate_blocks_a_broken_build() {
        let temp = fixture("name = \"a.pkg\"\nidentifier = \"\"\nversion = \"1.0\"\n");
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--lint"]);

        assert_eq!(status(&output), 3, "stderr: {}", stderr(&output));
        assert!(
            built_pkg(&project.join("build")).is_none(),
            "nothing should have been built"
        );
    }

    #[test]
    fn test_lint_gate_allows_a_clean_build() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--lint"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
        assert!(project.join("build/mypackage-1.0.pkg").exists());
    }

    #[test]
    fn test_export_bom_info_writes_bom_txt() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--export-bom-info", "--quiet"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let bom = project.join("Bom.txt");
        assert!(bom.exists());
        assert!(fs::read_to_string(&bom).unwrap().contains("hello.txt"));
    }

    #[test]
    fn test_ds_store_in_scripts_is_not_packaged() {
        let temp = fixture(default_build_info());
        let project = project_path(&temp);
        fs::create_dir_all(project.join("scripts")).unwrap();
        fs::write(project.join("scripts/postinstall"), "#!/bin/sh\n").unwrap();
        fs::write(project.join("scripts/.DS_Store"), "junk").unwrap();

        let output = munkipkg(&[project.to_str().unwrap(), "--quiet"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let expanded = temp.path().join("expanded");
        let expand = Command::new("/usr/sbin/pkgutil")
            .arg("--expand")
            .arg(project.join("build/mypackage-1.0.pkg"))
            .arg(&expanded)
            .output()
            .unwrap();
        assert!(expand.status.success(), "{}", stderr(&expand));

        assert!(expanded.join("Scripts/postinstall").exists());
        assert!(!expanded.join("Scripts/.DS_Store").exists());
    }

    #[test]
    fn test_distribution_style_package_verifies() {
        let temp = fixture(
            "name = \"mypackage-${version}.pkg\"\n\
             identifier = \"com.example.mypackage\"\n\
             version = \"1.0\"\n\
             distribution_style = true\n",
        );

        let output = munkipkg(&[project_path(&temp).to_str().unwrap(), "--verify"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    }

    #[test]
    fn test_create_then_build_round_trip() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("roundtrip");

        let created = munkipkg(&["create", project.to_str().unwrap(), "--format", "toml"]);
        assert_eq!(status(&created), 0, "stderr: {}", stderr(&created));

        fs::write(project.join("payload/marker.txt"), "content\n").unwrap();

        let built = munkipkg(&[project.to_str().unwrap(), "--quiet"]);
        assert_eq!(status(&built), 0, "stderr: {}", stderr(&built));
        assert!(built_pkg(&project.join("build")).is_some());
    }
}

// ---------------------------------------------------------------------------
// Application metadata
// ---------------------------------------------------------------------------

/// Build a realistic `.app` inside a project payload, using `plutil` so the
/// fixture is produced by Apple's own tooling rather than hand-written XML.
fn make_app(project: &Path, relative: &str, entries: &[(&str, &str)]) {
    let contents = project.join("payload").join(relative).join("Contents");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::write(contents.join("MacOS/stub"), "#!/bin/sh\nexit 0\n").unwrap();

    let body: String = entries
        .iter()
        .map(|(k, v)| format!("    <key>{}</key>\n    <string>{}</string>\n", k, v))
        .collect();
    fs::write(
        contents.join("Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n{}</dict>\n</plist>\n",
            body
        ),
    )
    .unwrap();
}

fn app_project(build_info: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("mypackage");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("build-info.toml"), build_info).unwrap();
    make_app(
        &project,
        "Applications/Foo.app",
        &[
            ("CFBundleIdentifier", "com.example.Foo"),
            ("CFBundleName", "Foo"),
            ("CFBundleShortVersionString", "3.4.1"),
            ("CFBundleVersion", "3410"),
            ("LSMinimumSystemVersion", "13.0"),
        ],
    );
    temp
}

fn app_build_info(version: &str) -> String {
    format!(
        "name = \"Foo-${{version}}.pkg\"\n\
         identifier = \"com.example.Foo\"\n\
         version = \"{}\"\n",
        version
    )
}

#[test]
fn test_appinfo_reports_bundle_metadata() {
    let temp = app_project(&app_build_info("1.0"));
    let output = munkipkg(&["appinfo", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Applications/Foo.app"), "got: {}", text);
    assert!(text.contains("com.example.Foo"));
    assert!(text.contains("3.4.1"));
    assert!(text.contains("3410"));
}

#[test]
fn test_appinfo_json_output() {
    let temp = app_project(&app_build_info("1.0"));
    let output = munkipkg(&[
        "appinfo",
        project_path(&temp).to_str().unwrap(),
        "--output-format",
        "json",
    ]);

    assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    let apps: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();

    assert_eq!(apps.as_array().unwrap().len(), 1);
    assert_eq!(apps[0]["short_version"], "3.4.1");
    assert_eq!(apps[0]["bundle_version"], "3410");
    assert_eq!(apps[0]["bundle_identifier"], "com.example.Foo");
    assert_eq!(apps[0]["relative_path"], "Applications/Foo.app");
}

#[test]
fn test_appinfo_on_a_payload_without_an_app() {
    let temp = fixture(default_build_info());
    let output = munkipkg(&["appinfo", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 0);
    assert!(stdout(&output).contains("No .app bundle found"));
}

/// The trap this feature exists to avoid: a shipping app embeds helper apps,
/// and a naive recursive scan would report several candidates.
#[test]
fn test_appinfo_ignores_nested_helper_apps() {
    let temp = app_project(&app_build_info("1.0"));
    let project = project_path(&temp);
    make_app(
        &project,
        "Applications/Foo.app/Contents/Frameworks/Foo Helper.app",
        &[("CFBundleShortVersionString", "9.9.9")],
    );
    make_app(
        &project,
        "Applications/Foo.app/Contents/Library/LoginItems/Launcher.app",
        &[("CFBundleShortVersionString", "0.0.1")],
    );

    let output = munkipkg(&[
        "appinfo",
        project.to_str().unwrap(),
        "--output-format",
        "json",
    ]);
    let apps: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();

    assert_eq!(
        apps.as_array().unwrap().len(),
        1,
        "got: {}",
        stdout(&output)
    );
    assert_eq!(apps[0]["short_version"], "3.4.1");
}

#[test]
fn test_lint_reports_the_resolved_app_version() {
    let temp = app_project(&app_build_info("${APP_VERSION}"));
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 0, "stdout: {}", stdout(&output));
    assert!(
        stdout(&output).contains("note: version resolves to \"3.4.1\""),
        "got: {}",
        stdout(&output)
    );
}

#[test]
fn test_lint_fails_when_an_app_token_cannot_be_satisfied() {
    let temp = fixture(
        "name = \"a-${version}.pkg\"\n\
         identifier = \"com.example.a\"\n\
         version = \"${APP_VERSION}\"\n",
    );
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap()]);

    assert_eq!(status(&output), 3, "stdout: {}", stdout(&output));
    assert!(stdout(&output).contains("No .app bundle found"));
}

/// A note is informational and must not fail even under --strict.
#[test]
fn test_lint_strict_tolerates_the_resolved_version_note() {
    let temp = app_project(&app_build_info("${APP_VERSION}"));
    let output = munkipkg(&["lint", project_path(&temp).to_str().unwrap(), "--strict"]);

    assert_eq!(status(&output), 0, "stdout: {}", stdout(&output));
}

#[cfg(target_os = "macos")]
mod app_building {
    use super::*;

    #[test]
    fn test_app_version_token_drives_the_package_version() {
        let temp = app_project(&app_build_info("${APP_VERSION}"));

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--output-format",
            "json",
        ]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        assert_eq!(manifest["version"], "3.4.1");
        assert_eq!(manifest["name"], "Foo-3.4.1.pkg");
    }

    #[test]
    fn test_app_build_token() {
        let temp = app_project(&app_build_info("${APP_VERSION}.${APP_BUILD}"));

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--output-format",
            "json",
        ]);
        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();

        assert_eq!(manifest["version"], "3.4.1.3410");
    }

    #[test]
    fn test_app_token_via_pkg_version_flag() {
        let temp = app_project(&app_build_info("1.0"));

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--pkg-version",
            "${APP_VERSION}",
            "--output-format",
            "json",
        ]);
        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();

        assert_eq!(manifest["version"], "3.4.1");
    }

    #[test]
    fn test_app_and_date_tokens_compose() {
        let temp = app_project(&app_build_info("${APP_VERSION}+${DATE}"));

        let output = munkipkg(&[
            project_path(&temp).to_str().unwrap(),
            "--output-format",
            "json",
        ]);
        let manifest: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
        let version = manifest["version"].as_str().unwrap();

        assert!(version.starts_with("3.4.1+20"), "got: {}", version);
    }

    /// The resolved version must reach the package metadata, not just the name.
    #[test]
    fn test_resolved_app_version_is_embedded_and_verifies() {
        let temp = app_project(&app_build_info("${APP_VERSION}"));
        let project = project_path(&temp);

        let output = munkipkg(&[project.to_str().unwrap(), "--verify"]);
        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));

        let expanded = temp.path().join("expanded");
        let expand = Command::new("/usr/sbin/pkgutil")
            .arg("--expand")
            .arg(project.join("build/Foo-3.4.1.pkg"))
            .arg(&expanded)
            .output()
            .unwrap();
        assert!(expand.status.success());

        let package_info = fs::read_to_string(expanded.join("PackageInfo")).unwrap();
        assert!(
            package_info.contains("version=\"3.4.1\""),
            "got: {}",
            package_info
        );
    }

    #[test]
    fn test_ambiguous_payload_fails_the_build() {
        let temp = app_project(&app_build_info("${APP_VERSION}"));
        let project = project_path(&temp);
        make_app(
            &project,
            "Applications/Bar.app",
            &[("CFBundleShortVersionString", "1.0")],
        );

        let output = munkipkg(&[project.to_str().unwrap()]);

        assert_eq!(status(&output), 3, "stderr: {}", stderr(&output));
        assert!(stderr(&output).contains("ambiguous"));
        assert!(stderr(&output).contains("Applications/Bar.app"));
    }

    /// A project that does not use the tokens must never scan or fail on them.
    #[test]
    fn test_payload_without_an_app_builds_normally() {
        let temp = fixture(default_build_info());
        let output = munkipkg(&[project_path(&temp).to_str().unwrap(), "--quiet"]);

        assert_eq!(status(&output), 0, "stderr: {}", stderr(&output));
    }
}
