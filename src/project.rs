//! Project creation and management
//!
//! Handles creating new munki-pkg project directories with
//! the standard structure and default configuration files.

use crate::config::{BuildInfo, OutputFormat};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

/// Default .gitignore content for new projects
const DEFAULT_GITIGNORE: &str = r#"# .DS_Store files!
.DS_Store

# Build output directory
build/
"#;

/// Create a new package project
pub fn create_project(project_dir: &Path, format: OutputFormat, force: bool) -> Result<()> {
    // Check if directory exists
    if project_dir.exists() {
        if force {
            println!("Removing existing project directory...");
            fs::remove_dir_all(project_dir)
                .with_context(|| format!("Failed to remove {}", project_dir.display()))?;
        } else {
            bail!(
                "Project directory already exists: {}. Use --force to overwrite.",
                project_dir.display()
            );
        }
    }

    // Get project name from directory name
    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled");

    println!("Creating project: {}", project_name);

    // Create directory structure
    fs::create_dir_all(project_dir)
        .with_context(|| format!("Failed to create {}", project_dir.display()))?;

    let payload_dir = project_dir.join("payload");
    fs::create_dir_all(&payload_dir)
        .with_context(|| format!("Failed to create {}", payload_dir.display()))?;

    let scripts_dir = project_dir.join("scripts");
    fs::create_dir_all(&scripts_dir)
        .with_context(|| format!("Failed to create {}", scripts_dir.display()))?;

    // Create build-info file
    let build_info = BuildInfo::new_project(project_name);
    build_info.save(project_dir, format)?;
    println!("Created {}", format.filename());

    // Create .gitignore
    let gitignore_path = project_dir.join(".gitignore");
    fs::write(&gitignore_path, DEFAULT_GITIGNORE)
        .with_context(|| format!("Failed to write {}", gitignore_path.display()))?;
    println!("Created .gitignore");

    println!(
        "\nProject created successfully at: {}",
        project_dir.display()
    );
    println!("\nNext steps:");
    println!("  1. Add files to the payload/ directory");
    println!("  2. Add scripts to the scripts/ directory (optional)");
    println!("  3. Edit {} to configure your package", format.filename());
    println!("  4. Run: munkipkg build {}", project_dir.display());

    Ok(())
}

/// Validate a project directory structure
pub fn validate_project(project_dir: &Path) -> Result<()> {
    if !project_dir.exists() {
        bail!(
            "Project directory does not exist: {}",
            project_dir.display()
        );
    }

    if !project_dir.is_dir() {
        bail!("Not a directory: {}", project_dir.display());
    }

    // Check for build-info file (any format)
    let build_info_files = [
        "build-info.plist",
        "build-info.json",
        "build-info.yaml",
        "build-info.toml",
    ];

    let has_build_info = build_info_files
        .iter()
        .any(|f| project_dir.join(f).exists());

    if !has_build_info {
        bail!(
            "No build-info file found in {}. Expected one of: {}",
            project_dir.display(),
            build_info_files.join(", ")
        );
    }

    Ok(())
}

/// Get the payload directory for a project
pub fn get_payload_dir(project_dir: &Path) -> Option<std::path::PathBuf> {
    let payload = project_dir.join("payload");
    if payload.exists() && payload.is_dir() {
        Some(payload)
    } else {
        None
    }
}

/// Get the scripts directory for a project
pub fn get_scripts_dir(project_dir: &Path) -> Option<std::path::PathBuf> {
    let scripts = project_dir.join("scripts");
    if scripts.exists() && scripts.is_dir() {
        Some(scripts)
    } else {
        None
    }
}

/// Check if a project has payload content
pub fn has_payload(project_dir: &Path) -> bool {
    if let Some(payload_dir) = get_payload_dir(project_dir) {
        // Check if payload directory has any files (excluding .DS_Store)
        if let Ok(entries) = fs::read_dir(&payload_dir) {
            return entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name() != ".DS_Store");
        }
    }
    false
}

/// Check if a project has scripts
pub fn has_scripts(project_dir: &Path) -> bool {
    if let Some(scripts_dir) = get_scripts_dir(project_dir)
        && let Ok(entries) = fs::read_dir(&scripts_dir)
    {
        return entries.filter_map(|e| e.ok()).any(|e| {
            let name = e.file_name();
            name == "preinstall" || name == "postinstall"
        });
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_project() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("TestProject");

        create_project(&project_dir, OutputFormat::Plist, false).unwrap();

        assert!(project_dir.exists());
        assert!(project_dir.join("payload").exists());
        assert!(project_dir.join("scripts").exists());
        assert!(project_dir.join("build-info.plist").exists());
        assert!(project_dir.join(".gitignore").exists());
    }

    #[test]
    fn test_validate_project() {
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("TestProject");

        // Should fail before creation
        assert!(validate_project(&project_dir).is_err());

        // Create and validate
        create_project(&project_dir, OutputFormat::Json, false).unwrap();
        assert!(validate_project(&project_dir).is_ok());
    }
}
