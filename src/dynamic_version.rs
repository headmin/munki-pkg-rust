//! Dynamic date and time tokens in a build-info `version` string.
//!
//! ```text
//! ${TIMESTAMP} -> yyyy.MM.dd.HHmm   (e.g. 2026.07.18.1405)
//! ${DATE}      -> yyyy.MM.dd        (e.g. 2026.07.18)
//! ${DATETIME}  -> yyyy.MM.dd.HHmmss (e.g. 2026.07.18.140530)
//! ```
//!
//! Tokens are stamped in local time, matching munki-pkg. None is a substring of
//! another, so replacement order does not matter.
//!
//! Two further tokens read the version out of an application in the payload,
//! so packaging an app does not mean restating its version by hand:
//!
//! ```text
//! ${APP_VERSION} -> CFBundleShortVersionString, e.g. 3.4.1
//! ${APP_BUILD}   -> CFBundleVersion, e.g. 3410
//! ```

use crate::appinfo::AppInfo;
use crate::clock::Timestamp;
use crate::errors::invalid_config;
use anyhow::Result;

/// Replace every dynamic token in `version` using the supplied clock reading.
pub fn resolve_at(version: &str, now: &Timestamp) -> String {
    if !version.contains("${") {
        return version.to_string();
    }

    version
        .replace("${TIMESTAMP}", &now.timestamp())
        .replace("${DATETIME}", &now.datetime())
        .replace("${DATE}", &now.date())
}

/// Replace every dynamic token in `version` using the current local time.
pub fn resolve(version: &str) -> String {
    resolve_at(version, &Timestamp::now_local())
}

/// Whether `version` references an application's own version.
///
/// Checked before scanning, so a project that does not use these tokens never
/// pays for the payload walk and never fails because of it.
pub fn contains_app_token(version: &str) -> bool {
    version.contains(APP_VERSION) || version.contains(APP_BUILD)
}

const APP_VERSION: &str = "${APP_VERSION}";
const APP_BUILD: &str = "${APP_BUILD}";

/// Replace the application tokens in `version` with values from `app`.
///
/// A token that the bundle cannot satisfy is an error rather than an empty
/// string: a package silently named `Foo-.pkg` is worse than a failed build.
pub fn resolve_app(version: &str, app: &AppInfo) -> Result<String> {
    let mut resolved = version.to_string();

    if resolved.contains(APP_VERSION) {
        let value = app.version().ok_or_else(|| {
            invalid_config(format!(
                "${{APP_VERSION}} was requested, but {} declares neither CFBundleShortVersionString nor CFBundleVersion",
                app.relative_path
            ))
        })?;
        resolved = resolved.replace(APP_VERSION, value);
    }

    if resolved.contains(APP_BUILD) {
        let value = app.bundle_version.as_deref().ok_or_else(|| {
            invalid_config(format!(
                "${{APP_BUILD}} was requested, but {} declares no CFBundleVersion",
                app.relative_path
            ))
        })?;
        resolved = resolved.replace(APP_BUILD, value);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Timestamp {
        Timestamp::new(2026, 7, 18, 14, 5, 30)
    }

    #[test]
    fn test_timestamp_token() {
        assert_eq!(resolve_at("${TIMESTAMP}", &at()), "2026.07.18.1405");
    }

    #[test]
    fn test_date_token() {
        assert_eq!(resolve_at("${DATE}", &at()), "2026.07.18");
    }

    #[test]
    fn test_datetime_token() {
        assert_eq!(resolve_at("${DATETIME}", &at()), "2026.07.18.140530");
    }

    #[test]
    fn test_token_inside_a_larger_version() {
        assert_eq!(resolve_at("2.1-${DATE}", &at()), "2.1-2026.07.18");
    }

    #[test]
    fn test_multiple_tokens_in_one_string() {
        assert_eq!(
            resolve_at("${DATE}+${TIMESTAMP}", &at()),
            "2026.07.18+2026.07.18.1405"
        );
    }

    /// `${DATE}` is a prefix of neither `${DATETIME}` nor `${TIMESTAMP}` as a
    /// whole token, but a naive ordered replace could still corrupt them. Pin
    /// the behavior so a future refactor cannot reintroduce that.
    #[test]
    fn test_date_replacement_does_not_corrupt_datetime() {
        assert_eq!(
            resolve_at("${DATETIME}/${DATE}", &at()),
            "2026.07.18.140530/2026.07.18"
        );
    }

    #[test]
    fn test_static_version_is_untouched() {
        assert_eq!(resolve_at("1.0", &at()), "1.0");
        assert_eq!(resolve_at("2026.07.18", &at()), "2026.07.18");
    }

    #[test]
    fn test_unknown_token_is_left_alone() {
        assert_eq!(resolve_at("${BUILD}", &at()), "${BUILD}");
    }

    #[test]
    fn test_version_placeholder_is_not_a_time_token() {
        assert_eq!(resolve_at("${version}", &at()), "${version}");
    }

    fn app(short: Option<&str>, build: Option<&str>) -> AppInfo {
        AppInfo {
            path: "payload/Applications/Foo.app".into(),
            relative_path: "Applications/Foo.app".to_string(),
            bundle_identifier: Some("com.example.Foo".to_string()),
            name: Some("Foo".to_string()),
            short_version: short.map(str::to_string),
            bundle_version: build.map(str::to_string),
            minimum_system_version: None,
        }
    }

    #[test]
    fn test_app_version_token() {
        let resolved = resolve_app("${APP_VERSION}", &app(Some("3.4.1"), Some("3410"))).unwrap();
        assert_eq!(resolved, "3.4.1");
    }

    #[test]
    fn test_app_build_token() {
        let resolved = resolve_app("${APP_BUILD}", &app(Some("3.4.1"), Some("3410"))).unwrap();
        assert_eq!(resolved, "3410");
    }

    #[test]
    fn test_app_tokens_combined() {
        let resolved = resolve_app(
            "${APP_VERSION}.${APP_BUILD}",
            &app(Some("3.4.1"), Some("3410")),
        )
        .unwrap();
        assert_eq!(resolved, "3.4.1.3410");
    }

    /// ${APP_VERSION} falls back to CFBundleVersion, matching AppInfo::version.
    #[test]
    fn test_app_version_falls_back_to_build_number() {
        let resolved = resolve_app("${APP_VERSION}", &app(None, Some("2024.1"))).unwrap();
        assert_eq!(resolved, "2024.1");
    }

    /// ${APP_BUILD} has no fallback: it names one specific key.
    #[test]
    fn test_app_build_without_bundle_version_is_an_error() {
        let error = resolve_app("${APP_BUILD}", &app(Some("3.4.1"), None)).unwrap_err();
        assert!(error.to_string().contains("no CFBundleVersion"));
    }

    #[test]
    fn test_app_version_with_no_version_at_all_is_an_error() {
        let error = resolve_app("${APP_VERSION}", &app(None, None)).unwrap_err();
        assert!(error.to_string().contains("Applications/Foo.app"));
    }

    #[test]
    fn test_app_tokens_are_detected() {
        assert!(contains_app_token("${APP_VERSION}"));
        assert!(contains_app_token("${APP_BUILD}"));
        assert!(contains_app_token("v${APP_VERSION}-release"));
        assert!(!contains_app_token("1.0"));
        assert!(!contains_app_token("${DATE}"));
        assert!(!contains_app_token("${version}"));
    }

    #[test]
    fn test_app_and_date_tokens_compose() {
        let resolved = resolve_app("${APP_VERSION}+${DATE}", &app(Some("3.4.1"), None)).unwrap();
        assert_eq!(resolve_at(&resolved, &at()), "3.4.1+2026.07.18");
    }
}
