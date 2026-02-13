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
mod bundle;
mod bundle_project;
mod config;
mod distribution;
mod external;
mod import;
mod project;
mod signing;
mod sync;

use build::build_package;
use bundle::build_bundle;
use bundle_project::create_bundle_project;
use config::OutputFormat;
use import::import_package;
use project::{create_project, select_format_interactive};
use signing::configure_signing_interactive;
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

    /// Configure signing/notarization before building
    #[arg(long)]
    configure: bool,
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

        /// Configure signing/notarization before building
        #[arg(long)]
        configure: bool,
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

        /// Configure signing after creation
        #[arg(long)]
        signing: bool,
    },

    /// Configure signing and notarization for a project
    Configure {
        /// Project directory to configure
        project_dir: PathBuf,
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

    /// Build or create multi-component distribution bundles
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },
}

#[derive(Subcommand)]
enum BundleAction {
    /// Build a distribution bundle from component packages
    Build {
        /// Bundle project directory containing bundle-info and components/
        bundle_dir: PathBuf,

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

        /// Configure signing/notarization before building
        #[arg(long)]
        configure: bool,
    },

    /// Create a new bundle project
    Create {
        /// Path for new bundle directory
        bundle_dir: PathBuf,

        /// Output format for bundle-info file
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,

        /// Overwrite existing bundle project
        #[arg(short, long)]
        force: bool,
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
            configure,
        }) => {
            if configure {
                configure_signing_interactive(&project_dir)?;
            }
            build_package(
                &project_dir,
                export_bom_info,
                quiet,
                skip_signing,
                skip_notarization,
                skip_stapling,
            )
        }

        Some(Commands::Create {
            project_dir,
            format,
            force,
            signing,
        }) => {
            let format = match format {
                Some(f) => f,
                None => select_format_interactive()?,
            };
            create_project(&project_dir, format, force)?;

            if signing {
                configure_signing_interactive(&project_dir)?;
            }
            Ok(())
        }

        Some(Commands::Configure { project_dir }) => configure_signing_interactive(&project_dir),

        Some(Commands::Import {
            pkg_path,
            project_dir,
            format,
        }) => import_package(&pkg_path, &project_dir, format),

        Some(Commands::Sync { project_dir }) => sync_from_bom(&project_dir),

        Some(Commands::Bundle { action }) => match action {
            BundleAction::Build {
                bundle_dir,
                quiet,
                skip_signing,
                skip_notarization,
                skip_stapling,
                configure,
            } => {
                if configure {
                    // TODO: bundle-level signing wizard (reuse signing.rs)
                    eprintln!("Bundle-level --configure not yet implemented. Configure signing in bundle-info directly.");
                }
                build_bundle(
                    &bundle_dir,
                    quiet,
                    skip_signing,
                    skip_notarization,
                    skip_stapling,
                )
            }

            BundleAction::Create {
                bundle_dir,
                format,
                force,
            } => {
                let format = match format {
                    Some(f) => f,
                    None => select_format_interactive()?,
                };
                create_bundle_project(&bundle_dir, format, force)
            }
        },

        None => {
            if let Some(project_dir) = cli.project_dir {
                if cli.configure {
                    configure_signing_interactive(&project_dir)?;
                }
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
