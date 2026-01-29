# munkipkg

A modern Rust port of [munki-pkg](https://github.com/munki/munki-pkg).

All credit for the original concept, design, and simplicity goes to **Greg Neagle**.

## Why

The original `munkipkg` is a Python script. This Rust port provides:

- **Compiled binary** - No Python dependency required
- **Code signed & notarized** - Ready for managed Mac environments
- **TOML support** - In addition to plist, JSON, and YAML
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

Credential formats supported:
- `op://vault/item/field` - 1Password CLI
- `@keychain:ITEM_NAME` - macOS Keychain
- `@env:VAR_NAME` - Environment variable or .env file

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
apple_id = "@env:APPLE_ID"
team_id = "@env:TEAM_ID"
password = "op://vault/notarization/password"
```

## Credits

All credit goes to **Greg Neagle** for his original ideas and workflows.
This is just a Rust port of [munki-pkg](https://github.com/munki/munki-pkg).
