//! BOM synchronization functionality
//!
//! Syncs file permissions and ownership from Bom.txt to the payload directory.
//! This is necessary because git doesn't track file permissions beyond executable bit.
//!
//! Original algorithm by Greg Neagle in munki-pkg.

use crate::project::{get_payload_dir, validate_project};
use anyhow::{Context, Result, bail};
use std::fs::{self, Permissions};
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::Path;

/// Bom entry representing a file's metadata
#[derive(Debug)]
pub struct BomEntry {
    /// Path relative to payload root (starts with ./)
    pub path: String,
    /// File mode as octal (e.g., 100644, 40755)
    pub mode: u32,
    /// User ID
    pub uid: u32,
    /// Group ID
    pub gid: u32,
}

impl BomEntry {
    /// Parse a line from Bom.txt
    /// Format: path<TAB>mode<TAB>uid/gid
    pub fn parse(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            return None;
        }

        let path = parts[0].to_string();
        let mode = u32::from_str_radix(parts[1], 8).ok()?;

        let uid_gid: Vec<&str> = parts[2].split('/').collect();
        if uid_gid.len() != 2 {
            return None;
        }

        let uid = uid_gid[0].parse().ok()?;
        let gid = uid_gid[1].parse().ok()?;

        Some(BomEntry {
            path,
            mode,
            uid,
            gid,
        })
    }

    /// Check if this entry represents a directory
    pub fn is_directory(&self) -> bool {
        // Mode starting with 4 indicates directory (040xxx)
        self.mode >= 0o40000 && self.mode < 0o50000
    }

    /// Get the unix file mode (lower 12 bits)
    pub fn file_mode(&self) -> u32 {
        self.mode & 0o7777
    }

    /// Get the path relative to payload (removes leading ./)
    pub fn relative_path(&self) -> &str {
        self.path.strip_prefix("./").unwrap_or(&self.path)
    }
}

/// Sync file modes and ownership from Bom.txt
pub fn sync_from_bom(project_dir: &Path) -> Result<()> {
    validate_project(project_dir)?;

    let bom_path = project_dir.join("Bom.txt");
    if !bom_path.exists() {
        bail!(
            "Bom.txt not found in {}. Run with --export-bom-info to create it.",
            project_dir.display()
        );
    }

    let payload_dir = get_payload_dir(project_dir).ok_or_else(|| {
        anyhow::anyhow!("No payload directory found in {}", project_dir.display())
    })?;

    println!("Syncing permissions from Bom.txt...");
    apply_bom_to_payload(&bom_path, &payload_dir)?;
    println!("Sync complete.");

    Ok(())
}

/// Apply BOM entries to payload directory
pub fn apply_bom_to_payload(bom_path: &Path, payload_dir: &Path) -> Result<()> {
    let content = fs::read_to_string(bom_path)
        .with_context(|| format!("Failed to read {}", bom_path.display()))?;

    let is_root = unsafe { libc::geteuid() } == 0;
    let mut errors = Vec::new();
    let mut synced = 0;
    let mut created_dirs = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let entry = match BomEntry::parse(line) {
            Some(e) => e,
            None => {
                eprintln!("Warning: Could not parse BOM line: {}", line);
                continue;
            }
        };

        let target_path = payload_dir.join(entry.relative_path());

        if entry.is_directory() {
            // Create directory if it doesn't exist
            if !target_path.exists() {
                if let Err(e) = fs::create_dir_all(&target_path) {
                    errors.push(format!(
                        "Failed to create directory {}: {}",
                        target_path.display(),
                        e
                    ));
                    continue;
                }
                created_dirs += 1;
            }
        }

        if target_path.exists() {
            // Set file mode
            let mode = entry.file_mode();
            if let Err(e) = fs::set_permissions(&target_path, Permissions::from_mode(mode)) {
                errors.push(format!(
                    "Failed to set mode on {}: {}",
                    target_path.display(),
                    e
                ));
                continue;
            }

            // Set ownership if running as root
            if is_root && let Err(e) = chown(&target_path, Some(entry.uid), Some(entry.gid)) {
                errors.push(format!(
                    "Failed to set ownership on {}: {}",
                    target_path.display(),
                    e
                ));
                continue;
            }

            synced += 1;
        }
    }

    println!("Synced {} files/directories", synced);
    if created_dirs > 0 {
        println!("Created {} missing directories", created_dirs);
    }

    if !is_root {
        println!("Note: Not running as root, ownership changes skipped");
    }

    if !errors.is_empty() {
        eprintln!("\nWarnings:");
        for error in &errors {
            eprintln!("  {}", error);
        }
    }

    Ok(())
}

/// Read BOM entries from a file
#[allow(dead_code)]
pub fn read_bom_entries(bom_path: &Path) -> Result<Vec<BomEntry>> {
    let content = fs::read_to_string(bom_path)
        .with_context(|| format!("Failed to read {}", bom_path.display()))?;

    let entries: Vec<BomEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(BomEntry::parse)
        .collect();

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bom_entry_parse() {
        let line = "./usr/local/bin/myapp\t100755\t0/0";
        let entry = BomEntry::parse(line).unwrap();

        assert_eq!(entry.path, "./usr/local/bin/myapp");
        assert_eq!(entry.mode, 0o100755);
        assert_eq!(entry.uid, 0);
        assert_eq!(entry.gid, 0);
    }

    #[test]
    fn test_bom_entry_directory() {
        let file_line = "./file\t100644\t501/20";
        let dir_line = "./dir\t40755\t0/0";

        let file_entry = BomEntry::parse(file_line).unwrap();
        let dir_entry = BomEntry::parse(dir_line).unwrap();

        assert!(!file_entry.is_directory());
        assert!(dir_entry.is_directory());
    }

    #[test]
    fn test_bom_entry_file_mode() {
        let line = "./file\t100644\t501/20";
        let entry = BomEntry::parse(line).unwrap();

        assert_eq!(entry.file_mode(), 0o644);
    }

    #[test]
    fn test_bom_entry_relative_path() {
        let line = "./usr/local/bin/myapp\t100755\t0/0";
        let entry = BomEntry::parse(line).unwrap();

        assert_eq!(entry.relative_path(), "usr/local/bin/myapp");
    }
}
