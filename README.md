# munkipkg

A modern Rust port of [munki-pkg](https://github.com/munki/munki-pkg).

All credit for the original concept, design, and simplicity goes to **Greg Neagle**.

## Why

The original `munkipkg` is a Python script. This Rust port provides:

- **Compiled binary** - No Python dependency required
- **Code signed & notarized** - Ready for managed Mac environments
- **TOML support** - In addition to plist, JSON, and YAML
- **Built for CI** - Lint gate, post-build verification, JSON manifests,
  provenance attestation, and failure-specific exit codes
- **Same workflow** - Drop-in compatible with existing projects

## Getting Started

### Install

Download the signed package from [Releases](https://github.com/headmin/munki-pkg-rust/releases):

```bash
sudo installer -pkg munkipkg-*.pkg -target /
```

### Create a new project

```bash
munkipkg create mypackage
```

Select your preferred format (TOML is default), then:

1. Add files to `mypackage/payload/`
2. Add scripts to `mypackage/scripts/` (optional)
3. Edit `mypackage/build-info.toml`

### Build a package

```bash
munkipkg build mypackage
```

Output: `mypackage/build/mypackage-1.0.pkg`

### Import an existing package

```bash
munkipkg import existing.pkg myproject
```

### Sync permissions after git clone

```bash
munkipkg sync mypackage
```

## SOPs

### Create and build a package

```bash
# Create project with interactive format selection
munkipkg create mypackage

# Add your files
cp -R /path/to/app mypackage/payload/Applications/

# Build
munkipkg mypackage
```

### Configure signing and notarization

```bash
# Interactive setup - prompts for identities and credentials
munkipkg create mypackage --signing

# Or configure an existing project
munkipkg configure mypackage
```

The wizard offers three ways to authenticate notarization:

**Keychain profile (recommended).** Credentials are stored once per machine by
`notarytool` itself, and build-info records only the profile name. Nothing
sensitive enters the repository or the environment. `munkipkg configure` can
create the profile for you — it runs `xcrun notarytool store-credentials` and
lets notarytool prompt for the app-specific password directly, so munkipkg never
sees it. To create one by hand:

```bash
xcrun notarytool store-credentials "AC_PASSWORD" \
    --apple-id "your-email@example.com" \
    --team-id "YOUR_TEAM_ID" \
    --password "abcd-efgh-ijkl-mnop"
```

Leave out `--password` to be prompted instead, so the app-specific password
stays out of your shell history. munkipkg then submits with
`notarytool submit ... --keychain-profile "AC_PASSWORD" --wait`, so no
environment variables are needed:

```toml
[notarization_info]
keychain_profile = "AC_PASSWORD"
```

**Apple ID and app-specific password**, with secret references so nothing is
committed in plain text:

- `op://vault/item/field` - 1Password CLI
- `@keychain:ITEM_NAME` - macOS Keychain
- `@env:VAR_NAME` - Environment variable or .env file

**App Store Connect API key** - `api_key_path`, `api_key_id`, `api_issuer_id`.

When more than one is configured, the keychain profile wins.

### Stray files in payload and scripts

`.DS_Store` files are removed from both `payload/` and `scripts/` on every
build, in a temporary copy, so your project directory is never touched. Extended
attributes (quarantine, provenance) are cleared recursively from both, which
matters most for downloaded apps and binaries. Set `preserve_xattr = true` to
keep the payload's xattrs; it also passes `--preserve-xattr` to `pkgbuild`.

### Reconfigure signing/notarization

```bash
# Reconfigure before building
munkipkg mypackage --configure
```

Select what to change:
- Application identity only (binary signing)
- Installer identity only (pkg signing)
- Notarization only
- Reconfigure all

### Build without signing/notarization

```bash
# Skip all signing
munkipkg mypackage --skip-signing

# Sign but don't notarize
munkipkg mypackage --skip-notarization
```

### Import an existing package

```bash
munkipkg import existing.pkg myproject --format toml
```

### Validate a project without building

`lint` loads build-info and checks the identifier, version, name, install
scripts, and signing coherence, then exits non-zero if anything would stop a
build. It runs in milliseconds, so it belongs on every pull request.

```bash
munkipkg lint mypackage
```

Add `--strict` to fail on warnings too, or `--quiet` to report only through the
exit status. To gate a build on it, pass `--lint`; the build stops before
pkgbuild rather than after a signing round trip:

```bash
munkipkg mypackage --lint
```

### Verify what was actually built

`--verify` re-reads the finished package and checks it against build-info: the
embedded identifier and version must match, `pkgutil --check-signature` must
pass when signing ran, and `spctl` must accept it when notarization ran.

```bash
munkipkg mypackage --verify
```

A stale artifact left in `build/`, or a version that silently did not take,
fails the build instead of shipping.

### Machine-readable build output

`--output-format json` reserves stdout for a manifest, so CI can consume the
result without scraping log lines. The booleans report what *happened*, not what
build-info requested.

```bash
munkipkg mypackage --output-format json
```

```json
{
  "name": "mypackage-1.0.pkg",
  "version": "1.0",
  "identifier": "com.example.mypackage",
  "pkg_path": "/path/to/mypackage/build/mypackage-1.0.pkg",
  "sha256": "50cdd862e304fd6ab17747830856628df64564e80c241bc1cfc3d28c538bd5e4",
  "signed": true,
  "notarized": true,
  "stapled": true
}
```

### Provenance attestation

`--provenance` writes `<pkg>.provenance.json` beside the package, recording the
tool version, build time, git commit and remote, a deterministic digest of the
build inputs, and the package hash.

```bash
munkipkg mypackage --provenance
```

The input digest covers each file's project-relative path, permission bits, and
contents, in sorted order — so it is stable across machines, and changes when
the payload, the scripts, or build-info change. Credentials embedded in a git
remote are stripped before the remote is recorded.

### Versioning from CI

Override the version from a git tag or pipeline variable, without editing
build-info:

```bash
munkipkg mypackage --pkg-version "${GITHUB_REF_NAME#v}"
```

build-info `version` also accepts dynamic tokens, stamped in local time:

| Token | Expands to | Example |
| --- | --- | --- |
| `${TIMESTAMP}` | `yyyy.MM.dd.HHmm` | `2026.07.18.1405` |
| `${DATE}` | `yyyy.MM.dd` | `2026.07.18` |
| `${DATETIME}` | `yyyy.MM.dd.HHmmss` | `2026.07.18.140530` |

The override is applied first, then tokens are stamped, then `${version}` in
`name` is substituted — so `--pkg-version '${DATE}'` works as expected.

### Packaging an application

When the payload contains an app, take the version from the app itself rather
than restating it:

| Token | Reads | Example |
| --- | --- | --- |
| `${APP_VERSION}` | `CFBundleShortVersionString` | `8.12.36` |
| `${APP_BUILD}` | `CFBundleVersion` | `3410` |

```toml
name = "1Password-${version}.pkg"
identifier = "com.1password.1password"
version = "${APP_VERSION}"
```

```bash
munkipkg 1password-project
# -> build/1Password-8.12.36.pkg
```

`${APP_VERSION}` falls back to `CFBundleVersion` when an app ships no
`CFBundleShortVersionString`. `${APP_BUILD}` names one key and has no fallback.
Both work through `--pkg-version` too, and compose with the date tokens
(`version = "${APP_VERSION}+${DATE}"`).

The scan walks `payload/` recursively for `.app` bundles and reads each one's
`Contents/Info.plist`, in XML or binary form. It does **not** descend into a
bundle it has already matched: shipping apps embed helper apps — 1Password
carries four under `Contents/Frameworks/` — and counting those would make every
such payload ambiguous.

Exactly one app must be present for the tokens to resolve. Two apps is an error
listing both, because guessing would silently stamp the package with the wrong
version. The payload is only scanned when a token is actually used, so projects
that do not package an app are unaffected.

Inspect what will be used before building:

```bash
munkipkg appinfo 1password-project
```

```text
Applications/1Password.app
  CFBundleIdentifier          com.1password.1password
  CFBundleName                1Password
  CFBundleShortVersionString  8.12.36
  CFBundleVersion             8.12.36
  LSMinimumSystemVersion      12.0
  ${APP_VERSION} resolves to  8.12.36
```

`--output-format json` reports the same thing as an array, for CI. `lint` also
resolves the token and reports the result as a note, so a payload that cannot
satisfy it fails the pre-check rather than the build.

### Writing the package elsewhere

```bash
munkipkg mypackage --output-dir dist
```

The directory is created if it does not exist. Without this flag the package
lands in the project's `build/` directory as usual.

### Exit codes

Each supported outcome returns a stable status, so a caller can branch on *why*
a build failed. Callers that only test for a non-zero status are unaffected.

| Status | Meaning |
| ---: | --- |
| 0 | Success |
| 1 | General or unclassified failure |
| 2 | Project already exists |
| 3 | Invalid configuration |
| 4 | Package import failure |
| 5 | Package build or subprocess failure |
| 6 | Package signing failure |
| 7 | Package notarization failure |
| 64 | Command-line usage error (`EX_USAGE`) |

## Configuration

Example `build-info.toml`:

```toml
name = "mypackage-${version}.pkg"
identifier = "com.example.mypackage"
version = "1.0"

install_location = "/"
ownership = "recommended"

distribution_style = false
suppress_bundle_relocation = true
postinstall_action = "none"

[signing_info]
identity = "Developer ID Application: Your Name (TEAMID)"
installer_identity = "Developer ID Installer: Your Name (TEAMID)"
timestamp = true

[notarization_info]
keychain_profile = "AC_PASSWORD"
```

Or, using secret references instead of a keychain profile:

```toml
[notarization_info]
apple_id = "@env:APPLE_ID"
team_id = "@env:TEAM_ID"
password = "op://vault/notarization/password"
```

## Continuous integration

```yaml
- name: Lint
  run: munkipkg lint packages/my-project --strict

- name: Build
  id: pkg
  run: |
    munkipkg packages/my-project \
      --pkg-version "${GITHUB_REF_NAME#v}" \
      --output-dir dist \
      --provenance \
      --verify \
      --output-format json > manifest.json
    echo "pkg-path=$(jq -r .pkg_path manifest.json)" >> "$GITHUB_OUTPUT"
    echo "sha256=$(jq -r .sha256 manifest.json)" >> "$GITHUB_OUTPUT"
```

## Development

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The test suite covers both the library internals and the built binary: the
end-to-end tests in `tests/cli.rs` run the real executable against Apple's
`pkgbuild` and assert on exit status, stdout, and the artifacts on disk. CI runs
all of it on macOS, then builds, verifies, and attests a real package as a smoke
test.

## Credits

All credit goes to **Greg Neagle** for his original ideas and workflows.
This is just a Rust port of [munki-pkg](https://github.com/munki/munki-pkg).
