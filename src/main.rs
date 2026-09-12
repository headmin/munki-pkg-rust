//! munkipkg - A modern Rust port of munki-pkg
//!
//! Original concept and design by Greg Neagle.
//! See: https://github.com/munki/munki-pkg
//!
//! This is a practical port to provide a compiled, signed binary
//! helping when building macOS installer packages.

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod appinfo;
mod build;
mod build_result;
mod bundle;
mod bundle_project;
mod clock;
mod config;
mod distribution;
mod dynamic_version;
mod errors;
mod external;
mod hash;
mod import;
mod lint;
mod project;
mod provenance;
mod signing;
mod sync;
mod verify;

use build::{BuildOptions, build_package};
use bundle::build_bundle;
use bundle_project::create_bundle_project;
use config::OutputFormat;
use errors::{EXIT_USAGE, exit_code_for, invalid_config};
use import::import_package;
use project::{create_project, select_format_interactive};
use signing::configure_signing_interactive;
use sync::sync_from_bom;

/// How the result of a build is reported on stdout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum ResultFormat {
    /// Human-readable status lines.
    #[default]
    Text,
    /// A machine-readable JSON manifest, and nothing else.
    Json,
}

/// Options shared by every path that builds a package.
///
/// Flattened into both the bare `munkipkg PROJECT` form and the explicit
/// `munkipkg build PROJECT` subcommand, so the two can never drift apart.
#[derive(Debug, Args, Clone, Default)]
struct BuildArgs {
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

    /// Validate the project first and refuse to build if anything is wrong
    #[arg(long)]
    lint: bool,

    /// After building, check the package's identifier and version against
    /// build-info, its signature when signing ran, and Gatekeeper acceptance
    /// when notarization ran
    #[arg(long)]
    verify: bool,

    /// Write a <pkg>.provenance.json attestation beside the package
    #[arg(long)]
    provenance: bool,

    /// Result printed to stdout after a build. `json` implies --quiet so
    /// stdout carries only the manifest
    #[arg(long, value_enum, default_value_t = ResultFormat::Text)]
    output_format: ResultFormat,

    /// Override the build-info version, e.g. from a git tag or CI variable.
    /// Resolved before ${version} substitution
    #[arg(long, value_name = "VERSION")]
    pkg_version: Option<String>,

    /// Write the package to this directory instead of the project's build/
    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,
}

impl BuildArgs {
    /// `--output-format json` reserves stdout for the manifest.
    fn is_quiet(&self) -> bool {
        self.quiet || self.output_format == ResultFormat::Json
    }

    fn to_options(&self) -> BuildOptions {
        BuildOptions {
            export_bom: self.export_bom_info,
            quiet: self.is_quiet(),
            skip_signing: self.skip_signing,
            skip_notarization: self.skip_notarization,
            skip_stapling: self.skip_stapling,
            provenance: self.provenance,
            verify: self.verify,
            version_override: self.pkg_version.clone(),
            output_dir: self.output_dir.clone(),
        }
    }
}

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

    #[command(flatten)]
    build: BuildArgs,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a package from a project directory
    Build {
        /// Project directory containing build-info and payload
        project_dir: PathBuf,

        #[command(flatten)]
        build: BuildArgs,
    },

    /// Validate a project without building it
    ///
    /// Loads build-info and checks the identifier, version, name, install
    /// scripts and signing coherence, then exits non-zero if anything would
    /// stop a build. A fast pre-check for pull-request CI.
    Lint {
        /// Project directory to validate
        project_dir: PathBuf,

        /// Treat warnings as errors
        #[arg(long)]
        strict: bool,

        /// Suppress output; report only through the exit status
        #[arg(short, long)]
        quiet: bool,
    },

    /// Show the application metadata found in a project's payload
    ///
    /// Recursively scans `payload/` for `.app` bundles and reports what each
    /// one declares in its `Info.plist`. Use it to see what `${APP_VERSION}`
    /// and `${APP_BUILD}` will resolve to before a build.
    Appinfo {
        /// Project directory whose payload should be scanned
        project_dir: PathBuf,

        /// Report as JSON instead of human-readable lines
        #[arg(long, value_enum, default_value_t = ResultFormat::Text)]
        output_format: ResultFormat,
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

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return match error.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    ExitCode::SUCCESS
                }
                _ => ExitCode::from(EXIT_USAGE),
            };
        }
    };

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {:#}", error);
            ExitCode::from(exit_code_for(&error))
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Commands::Build { project_dir, build }) => run_build(&project_dir, &build),

        Some(Commands::Lint {
            project_dir,
            strict,
            quiet,
        }) => run_lint(&project_dir, strict, quiet),

        Some(Commands::Appinfo {
            project_dir,
            output_format,
        }) => run_appinfo(&project_dir, output_format),

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
                    eprintln!(
                        "Bundle-level --configure not yet implemented. Configure signing in bundle-info directly."
                    );
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

        None => match cli.project_dir {
            Some(project_dir) => run_build(&project_dir, &cli.build),
            None => Err(invalid_config(
                "No project directory specified. Use --help for usage information.",
            )
            .into()),
        },
    }
}

/// Build one project, honoring every build-time flag.
fn run_build(project_dir: &Path, args: &BuildArgs) -> Result<()> {
    if args.configure {
        configure_signing_interactive(project_dir)?;
    }

    // Lint first when asked, so a misconfigured project fails in seconds rather
    // than after pkgbuild, signing, and a notarization round trip.
    if args.lint {
        run_lint(project_dir, false, args.is_quiet())?;
    }

    let result = build_package(project_dir, &args.to_options())?;

    if args.output_format == ResultFormat::Json {
        println!("{}", result.to_json()?);
    }

    Ok(())
}

/// Validate a project and fail if anything would stop a build.
fn run_lint(project_dir: &Path, strict: bool, quiet: bool) -> Result<()> {
    let findings = lint::lint(project_dir)?;

    if !quiet {
        for finding in &findings {
            println!("{}", finding);
        }
    }

    let count = |severity| findings.iter().filter(|f| f.severity == severity).count();
    let errors = count(lint::Severity::Error);
    let warnings = count(lint::Severity::Warning);

    if lint::has_errors(&findings) || (strict && warnings > 0) {
        return Err(invalid_config(format!(
            "{} failed lint: {} error(s), {} warning(s)",
            project_dir.display(),
            errors,
            warnings
        ))
        .into());
    }

    if !quiet {
        println!(
            "{}: {}",
            project_dir.display(),
            if findings.is_empty() {
                "no findings".to_string()
            } else {
                format!("no errors, {} warning(s)", warnings)
            }
        );
    }

    Ok(())
}

/// Report the application bundles found in a project's payload.
fn run_appinfo(project_dir: &Path, output_format: ResultFormat) -> Result<()> {
    let payload = project_dir.join("payload");
    let apps = appinfo::scan(&payload)?;

    if output_format == ResultFormat::Json {
        println!("{}", serde_json::to_string_pretty(&apps)?);
        return Ok(());
    }

    if apps.is_empty() {
        println!("No .app bundle found under {}", payload.display());
        return Ok(());
    }

    for (index, app) in apps.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{}", app.relative_path);
        let field = |label: &str, value: Option<&str>| {
            println!("  {:<27} {}", label, value.unwrap_or("-"));
        };
        field("CFBundleIdentifier", app.bundle_identifier.as_deref());
        field("CFBundleName", app.name.as_deref());
        field("CFBundleShortVersionString", app.short_version.as_deref());
        field("CFBundleVersion", app.bundle_version.as_deref());
        field(
            "LSMinimumSystemVersion",
            app.minimum_system_version.as_deref(),
        );
        field("${APP_VERSION} resolves to", app.version());
    }

    if apps.len() > 1 {
        println!(
            "\n{} bundles found; ${{APP_VERSION}} needs exactly one. \
             Set `version` explicitly, or pass --pkg-version.",
            apps.len()
        );
    }

    Ok(())
}
