//! munkipkg - A modern Rust port of munki-pkg
//!
//! Original concept and design by Greg Neagle.
//! See: https://github.com/munki/munki-pkg
//!
//! This is a practical port to provide a compiled, signed binary
//! helping when building macOS installer packages.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod build;
mod config;
mod external;
mod import;
mod project;
mod sync;

use build::build_package;
use config::OutputFormat;
use import::import_package;
use project::{create_project, select_format_interactive};
use sync::sync_from_bom;

/// munkipkg - Build macOS installer packages
///
/// A modern Rust port of munki-pkg, original design all courtesy of Greg Neagle.
/// See the original project at https://github.com/munki/munki-pkg
#[derive(Parser)]
#[command(name = "munkipkg")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Project directory to build (when no subcommand given)
    #[arg(value_name = "PROJECT_DIR")]
    project_dir: Option<PathBuf>,

    /// Export bom info after building
    #[arg(long)]
    export_bom_info: bool,

    /// Suppress output messages
    #[arg(short, long)]
    quiet: bool,

    /// Skip package signing
    #[arg(long)]
    skip_signing: bool,

    /// Skip notarization
    #[arg(long)]
    skip_notarization: bool,

    /// Skip stapling after notarization
    #[arg(long)]
    skip_stapling: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a package from a project directory
    Build {
        /// Project directory containing build-info and payload
        project_dir: PathBuf,

        /// Export bom info after building
        #[arg(long)]
        export_bom_info: bool,

        /// Suppress output messages
        #[arg(short, long)]
        quiet: bool,

        /// Skip package signing
        #[arg(long)]
        skip_signing: bool,

        /// Skip notarization
        #[arg(long)]
        skip_notarization: bool,

        /// Skip stapling after notarization
        #[arg(long)]
        skip_stapling: bool,
    },

    /// Create a new package project
    Create {
        /// Path for new project directory
        project_dir: PathBuf,

        /// Output format for build-info file (prompts if not specified)
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,

        /// Overwrite existing project
        #[arg(short, long)]
        force: bool,
    },

    /// Import an existing package into a project
    Import {
        /// Path to existing .pkg file
        pkg_path: PathBuf,

        /// Path for new project directory
        project_dir: PathBuf,

        /// Output format for build-info file
        #[arg(long, value_enum, default_value = "toml")]
        format: OutputFormat,
    },

    /// Sync file modes and ownership from Bom.txt
    Sync {
        /// Project directory containing Bom.txt
        project_dir: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Build {
            project_dir,
            export_bom_info,
            quiet,
            skip_signing,
            skip_notarization,
            skip_stapling,
        }) => build_package(
            &project_dir,
            export_bom_info,
            quiet,
            skip_signing,
            skip_notarization,
            skip_stapling,
        ),

        Some(Commands::Create {
            project_dir,
            format,
            force,
        }) => {
            let format = match format {
                Some(f) => f,
                None => select_format_interactive()?,
            };
            create_project(&project_dir, format, force)
        }

        Some(Commands::Import {
            pkg_path,
            project_dir,
            format,
        }) => import_package(&pkg_path, &project_dir, format),

        Some(Commands::Sync { project_dir }) => sync_from_bom(&project_dir),

        None => {
            if let Some(project_dir) = cli.project_dir {
                build_package(
                    &project_dir,
                    cli.export_bom_info,
                    cli.quiet,
                    cli.skip_signing,
                    cli.skip_notarization,
                    cli.skip_stapling,
                )
            } else {
                eprintln!("Error: No project directory specified.");
                eprintln!("Use --help for usage information.");
                std::process::exit(1);
            }
        }
    }
}
