//! Typed errors and their stable process exit codes.
//!
//! The CLI returns a failure-specific status for each supported outcome so
//! callers can branch on *why* a build failed without scraping stderr. Callers
//! that only test for a non-zero status are unaffected.
//!
//! | Status | Meaning |
//! | -----: | ------- |
//! |      0 | Success |
//! |      1 | General or unclassified failure |
//! |      2 | Project already exists |
//! |      3 | Invalid configuration |
//! |      4 | Package import failure |
//! |      5 | Package build or subprocess failure |
//! |      6 | Package signing failure |
//! |      7 | Package notarization failure |
//! |     64 | Command-line usage error (`EX_USAGE`) |

use thiserror::Error;

/// Exit status for a command-line usage error, matching `sysexits.h` `EX_USAGE`.
pub const EXIT_USAGE: u8 = 64;

/// An error carrying the exit code the process should return.
///
/// Wrap these in `anyhow::Context` freely — [`exit_code_for`] walks the whole
/// error chain, so added context never loses the code.
///
/// Status 1 is deliberately absent: it is what [`exit_code_for`] returns for
/// any error that is not one of these, so there is nothing to construct.
#[derive(Debug, Error)]
pub enum MunkiPkgError {
    /// The target project directory already exists and `--force` was not given.
    #[error("{0}")]
    ProjectExists(String),

    /// build-info is missing, unparseable, or internally inconsistent.
    #[error("{0}")]
    InvalidConfiguration(String),

    /// An existing package could not be imported into a project.
    #[error("{0}")]
    ImportFailed(String),

    /// The package could not be built, or a subprocess failed.
    #[error("{0}")]
    BuildFailed(String),

    /// The package could not be signed.
    #[error("{0}")]
    SigningFailed(String),

    /// The package could not be notarized or stapled.
    #[error("{0}")]
    NotarizationFailed(String),
}

impl MunkiPkgError {
    /// The process exit status for this error.
    pub fn exit_code(&self) -> u8 {
        match self {
            MunkiPkgError::ProjectExists(_) => 2,
            MunkiPkgError::InvalidConfiguration(_) => 3,
            MunkiPkgError::ImportFailed(_) => 4,
            MunkiPkgError::BuildFailed(_) => 5,
            MunkiPkgError::SigningFailed(_) => 6,
            MunkiPkgError::NotarizationFailed(_) => 7,
        }
    }
}

/// The exit status for any error leaving the CLI.
///
/// Walks the `anyhow` chain so a [`MunkiPkgError`] still sets the status after
/// being wrapped with `.context(...)`. Anything else is an unclassified `1`.
pub fn exit_code_for(error: &anyhow::Error) -> u8 {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<MunkiPkgError>())
        .map(MunkiPkgError::exit_code)
        .unwrap_or(1)
}

/// Shorthand constructors, so call sites read as prose.
macro_rules! ctor {
    ($name:ident, $variant:ident, $doc:literal) => {
        #[doc = $doc]
        pub fn $name(message: impl Into<String>) -> MunkiPkgError {
            MunkiPkgError::$variant(message.into())
        }
    };
}

ctor!(
    project_exists,
    ProjectExists,
    "The project directory already exists (status 2)."
);
ctor!(
    invalid_config,
    InvalidConfiguration,
    "build-info is missing or invalid (status 3)."
);
ctor!(
    import_failed,
    ImportFailed,
    "A package could not be imported (status 4)."
);
ctor!(
    build_failed,
    BuildFailed,
    "The build or a subprocess failed (status 5)."
);
ctor!(signing_failed, SigningFailed, "Signing failed (status 6).");
ctor!(
    notarization_failed,
    NotarizationFailed,
    "Notarization or stapling failed (status 7)."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_codes_match_the_documented_ladder() {
        assert_eq!(project_exists("x").exit_code(), 2);
        assert_eq!(invalid_config("x").exit_code(), 3);
        assert_eq!(import_failed("x").exit_code(), 4);
        assert_eq!(build_failed("x").exit_code(), 5);
        assert_eq!(signing_failed("x").exit_code(), 6);
        assert_eq!(notarization_failed("x").exit_code(), 7);
        assert_eq!(EXIT_USAGE, 64);
    }

    #[test]
    fn test_exit_code_survives_context_wrapping() {
        let error = anyhow::Error::from(notarization_failed("ticket never arrived"))
            .context("while notarizing")
            .context("while building mypackage");

        assert_eq!(exit_code_for(&error), 7);
    }

    #[test]
    fn test_untyped_errors_are_unclassified() {
        let error = anyhow::anyhow!("something went sideways");
        assert_eq!(exit_code_for(&error), 1);
    }

    #[test]
    fn test_message_is_preserved() {
        let error = invalid_config("version is empty");
        assert_eq!(error.to_string(), "version is empty");
    }
}
