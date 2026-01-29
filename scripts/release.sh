#!/bin/bash
# release.sh - Build, sign, notarize, and release munkipkg
#
# This script builds munkipkg, signs it, notarizes it, creates a pkg
# using munkipkg itself (dogfooding!), and optionally creates a GitHub release.
#
# Original munkipkg concept by Greg Neagle: https://github.com/munki/munki-pkg

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
PKG_PROJECT="$PROJECT_ROOT/pkg"
BUILD_DIR="$PROJECT_ROOT/target/release"
BINARY_NAME="munkipkg"

# Load .env file if it exists
if [[ -f "$SCRIPT_DIR/.env" ]]; then
    # shellcheck source=/dev/null
    source "$SCRIPT_DIR/.env"
elif [[ -f "$PROJECT_ROOT/.env" ]]; then
    # shellcheck source=/dev/null
    source "$PROJECT_ROOT/.env"
fi

# Signing identities (from Keychain) - use env vars or prompt
SIGNING_IDENTITY="${SIGNING_IDENTITY:-}"
INSTALLER_IDENTITY="${INSTALLER_IDENTITY:-}"
NOTARIZE_PROFILE="${NOTARIZE_PROFILE:-}"

# 1Password references for notarization
OP_APPLE_ID="${OP_APPLE_ID:-op://dev-credentials/NOTARIZATION_APPLE_ID/credential}"
OP_PASSWORD="${OP_PASSWORD:-op://dev-credentials/NOTARIZATION_PASSWORD/credential}"
OP_TEAM_ID="${OP_TEAM_ID:-op://dev-credentials/NOTARIZATION_TEAM_ID/credential}"
USE_1PASSWORD="${USE_1PASSWORD:-false}"

# Version from Cargo.toml
VERSION=$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
BUMP_TYPE=""

usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Build, sign, notarize, and release munkipkg"
    echo ""
    echo "Configuration:"
    echo "  Create a .env file in scripts/ or project root with:"
    echo "    SIGNING_IDENTITY, INSTALLER_IDENTITY, NOTARIZE_PROFILE,"
    echo "    OP_APPLE_ID, OP_PASSWORD, OP_TEAM_ID, USE_1PASSWORD"
    echo "  See .env.example for reference."
    echo ""
    echo "Options:"
    echo "  --bump TYPE           Bump version before release (major|minor|patch)"
    echo "  --sign IDENTITY       Code signing identity (overrides .env)"
    echo "  --notarize PROFILE    Notarization profile name (from notarytool store-credentials)"
    echo "  --op                  Use 1Password CLI to fetch notarization credentials"
    echo "  --github              Create GitHub release after building"
    echo "  --version VERSION     Set exact version (default: from Cargo.toml)"
    echo "  --skip-notarize       Skip notarization step"
    echo "  --help                Show this help"
    echo ""
    echo "Examples:"
    echo "  $0 --bump patch --op --github    # Bump patch, build, sign, notarize, release"
    echo "  $0 --bump minor --op             # Bump minor version and release"
    echo "  $0 --op --github                 # Release current version"
    echo "  $0 --skip-notarize               # Build and sign only"
}

# Bump version number
bump_version() {
    local current="$1"
    local type="$2"

    IFS='.' read -r major minor patch <<< "$current"

    case "$type" in
        major)
            major=$((major + 1))
            minor=0
            patch=0
            ;;
        minor)
            minor=$((minor + 1))
            patch=0
            ;;
        patch)
            patch=$((patch + 1))
            ;;
        *)
            error "Invalid bump type: $type (use major, minor, or patch)"
            ;;
    esac

    echo "${major}.${minor}.${patch}"
}

# Update version in files
update_version_files() {
    local new_version="$1"

    log "Updating version to $new_version..."

    # Update Cargo.toml
    sed -i '' "s/^version = \".*\"/version = \"$new_version\"/" "$PROJECT_ROOT/Cargo.toml"

    # Update pkg/build-info.toml
    if [[ -f "$PKG_PROJECT/build-info.toml" ]]; then
        sed -i '' "s/^version = \".*\"/version = \"$new_version\"/" "$PKG_PROJECT/build-info.toml"
    fi

    log "Version updated in Cargo.toml and pkg/build-info.toml"
}

log() {
    echo -e "${GREEN}==>${NC} $1"
}

warn() {
    echo -e "${YELLOW}Warning:${NC} $1"
}

error() {
    echo -e "${RED}Error:${NC} $1"
    exit 1
}

# Parse arguments
GITHUB_RELEASE=false
SKIP_NOTARIZE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --bump)
            BUMP_TYPE="$2"
            shift 2
            ;;
        --sign)
            SIGNING_IDENTITY="$2"
            shift 2
            ;;
        --notarize)
            NOTARIZE_PROFILE="$2"
            shift 2
            ;;
        --op)
            USE_1PASSWORD=true
            shift
            ;;
        --github)
            GITHUB_RELEASE=true
            shift
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        --skip-notarize)
            SKIP_NOTARIZE=true
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            error "Unknown option: $1"
            ;;
    esac
done

# Bump version if requested
if [[ -n "$BUMP_TYPE" ]]; then
    NEW_VERSION=$(bump_version "$VERSION" "$BUMP_TYPE")
    update_version_files "$NEW_VERSION"
    VERSION="$NEW_VERSION"
fi

log "munkipkg Release Script v${VERSION}"
echo ""

# Check requirements
command -v cargo >/dev/null 2>&1 || error "cargo not found. Install Rust first."
command -v codesign >/dev/null 2>&1 || error "codesign not found. Xcode required."

if [[ "$GITHUB_RELEASE" == "true" ]]; then
    command -v gh >/dev/null 2>&1 || error "gh (GitHub CLI) not found. Install with: brew install gh"
fi

# Function to select signing identity
select_signing_identity() {
    local identity_type="$1"  # "Application" or "Installer"
    local var_name="$2"       # Variable name to set

    # Get available identities
    local identities
    identities=$(security find-identity -v -p codesigning 2>/dev/null | grep "Developer ID $identity_type" | sed 's/^[[:space:]]*[0-9]*)[[:space:]]*//' | sed 's/[[:space:]]*$//')

    if [[ -z "$identities" ]]; then
        warn "No Developer ID $identity_type certificates found in Keychain"
        return 1
    fi

    # Count identities
    local count
    count=$(echo "$identities" | wc -l | tr -d ' ')

    if [[ "$count" -eq 1 ]]; then
        # Only one identity, use it automatically
        local identity
        identity=$(echo "$identities" | sed 's/^[A-F0-9]* "//' | sed 's/"$//')
        log "Found signing identity: $identity"
        eval "$var_name=\"$identity\""
        return 0
    fi

    # Multiple identities, prompt user to choose
    echo ""
    log "Multiple Developer ID $identity_type certificates found:"
    echo ""

    local i=1
    local identity_array=()
    while IFS= read -r line; do
        local identity
        identity=$(echo "$line" | sed 's/^[A-F0-9]* "//' | sed 's/"$//')
        identity_array+=("$identity")
        echo "  $i) $identity"
        ((i++))
    done <<< "$identities"

    echo ""
    read -rp "Select identity (1-$count): " choice

    if [[ "$choice" -ge 1 ]] && [[ "$choice" -le "$count" ]]; then
        eval "$var_name=\"${identity_array[$((choice-1))]}\""
        log "Selected: ${identity_array[$((choice-1))]}"
        return 0
    else
        error "Invalid selection"
    fi
}

# Select signing identities if not provided
if [[ -z "$SIGNING_IDENTITY" ]]; then
    select_signing_identity "Application" "SIGNING_IDENTITY" || true
fi

if [[ -z "$INSTALLER_IDENTITY" ]]; then
    select_signing_identity "Installer" "INSTALLER_IDENTITY" || true
fi

# Step 1: Build release binary
log "Building release binary..."
cd "$PROJECT_ROOT"
cargo build --release

# Verify binary exists
[[ -f "$BUILD_DIR/$BINARY_NAME" ]] || error "Binary not found at $BUILD_DIR/$BINARY_NAME"

log "Binary built: $BUILD_DIR/$BINARY_NAME ($(du -h "$BUILD_DIR/$BINARY_NAME" | cut -f1))"

# Step 2: Sign the binary
if [[ -n "$SIGNING_IDENTITY" ]]; then
    log "Signing binary with identity: $SIGNING_IDENTITY"
    codesign --force --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" \
        "$BUILD_DIR/$BINARY_NAME"

    log "Verifying signature..."
    codesign --verify --verbose "$BUILD_DIR/$BINARY_NAME"
else
    warn "No signing identity provided. Binary will not be signed."
    warn "Use --sign 'Developer ID Application: ...' to sign"
fi

# Step 3: Prepare pkg project payload
log "Preparing pkg project..."
mkdir -p "$PKG_PROJECT/payload/usr/local/bin"
mkdir -p "$PKG_PROJECT/scripts"

# Copy binary to payload
cp "$BUILD_DIR/$BINARY_NAME" "$PKG_PROJECT/payload/usr/local/bin/"
chmod 755 "$PKG_PROJECT/payload/usr/local/bin/$BINARY_NAME"

# Step 4: Build the pkg using munkipkg itself (dogfooding!)
log "Building pkg using munkipkg (dogfooding!)..."
"$BUILD_DIR/$BINARY_NAME" build "$PKG_PROJECT"

PKG_PATH="$PKG_PROJECT/build/munkipkg-${VERSION}.pkg"
[[ -f "$PKG_PATH" ]] || error "Package not created at $PKG_PATH"

log "Package built: $PKG_PATH"

# Step 5: Sign the pkg
if [[ -n "$INSTALLER_IDENTITY" ]]; then
    log "Signing package with: $INSTALLER_IDENTITY"

    SIGNED_PKG="$PKG_PROJECT/build/munkipkg-${VERSION}-signed.pkg"
    productsign --sign "$INSTALLER_IDENTITY" --timestamp \
        "$PKG_PATH" "$SIGNED_PKG"

    # Replace unsigned with signed
    mv "$SIGNED_PKG" "$PKG_PATH"

    log "Package signed successfully"
fi

# Step 6: Notarize
if [[ "$SKIP_NOTARIZE" == "true" ]]; then
    warn "Skipping notarization as requested"
elif [[ "$USE_1PASSWORD" == "true" ]]; then
    log "Fetching notarization credentials from 1Password..."
    command -v op >/dev/null 2>&1 || error "1Password CLI (op) not found. Install with: brew install 1password-cli"

    APPLE_ID=$(op read "$OP_APPLE_ID")
    APP_PASSWORD=$(op read "$OP_PASSWORD")
    TEAM_ID=$(op read "$OP_TEAM_ID")

    log "Submitting package for notarization..."
    xcrun notarytool submit "$PKG_PATH" \
        --apple-id "$APPLE_ID" \
        --password "$APP_PASSWORD" \
        --team-id "$TEAM_ID" \
        --wait

    log "Stapling notarization ticket..."
    xcrun stapler staple "$PKG_PATH"

    log "Package notarized and stapled successfully"
elif [[ -n "$NOTARIZE_PROFILE" ]]; then
    log "Submitting package for notarization..."

    xcrun notarytool submit "$PKG_PATH" \
        --keychain-profile "$NOTARIZE_PROFILE" \
        --wait

    log "Stapling notarization ticket..."
    xcrun stapler staple "$PKG_PATH"

    log "Package notarized and stapled successfully"
else
    warn "No notarization method provided. Package will not be notarized."
    warn "Use --op for 1Password or --notarize PROFILE for keychain"
fi

# Step 7: Create GitHub release
if [[ "$GITHUB_RELEASE" == "true" ]]; then
    log "Creating GitHub release v${VERSION}..."

    # Check if tag exists
    if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
        warn "Tag v${VERSION} already exists"
    else
        log "Creating tag v${VERSION}..."
        git tag -a "v${VERSION}" -m "Release v${VERSION}"
        git push origin "v${VERSION}"
    fi

    # Create release with pkg as asset
    log "Creating GitHub release..."
    gh release create "v${VERSION}" \
        --title "munkipkg v${VERSION}" \
        --notes "## munkipkg v${VERSION}

A modern Rust port of [munki-pkg](https://github.com/munki/munki-pkg) by Greg Neagle.

### Installation

Download the signed and notarized installer package:

\`\`\`bash
# Install via pkg
sudo installer -pkg munkipkg-${VERSION}.pkg -target /
\`\`\`

Or download the binary directly and place it in your PATH.

### Changes

- See commit history for details

### Credits

All credit for the original munkipkg concept and design goes to **Greg Neagle**.
This is simply a Rust port to provide a compiled, signed binary for automation." \
        "$PKG_PATH"

    log "GitHub release created: https://github.com/headmin/munki-pkg-rust/releases/tag/v${VERSION}"
fi

# Summary
echo ""
log "Release complete!"
echo ""
echo "Artifacts:"
echo "  Binary: $BUILD_DIR/$BINARY_NAME"
echo "  Package: $PKG_PATH"
echo ""

if [[ -n "$SIGNING_IDENTITY" ]]; then
    echo "  ✓ Binary signed"
    echo "  ✓ Package signed"
else
    echo "  ✗ Not signed (use --op or --sign)"
fi

if [[ "$USE_1PASSWORD" == "true" ]] || [[ -n "$NOTARIZE_PROFILE" ]]; then
    if [[ "$SKIP_NOTARIZE" != "true" ]]; then
        echo "  ✓ Notarized"
    else
        echo "  ✗ Not notarized (skipped)"
    fi
else
    echo "  ✗ Not notarized (use --op or --notarize)"
fi

if [[ "$GITHUB_RELEASE" == "true" ]]; then
    echo "  ✓ GitHub release created"
fi
