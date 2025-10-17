#!/usr/bin/env bash
set -euo pipefail

# LunaRoute Release Script
# Triggers a GitHub Actions release build for Windows, macOS, and Linux

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║         LunaRoute Release Builder                        ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════╝${NC}"
echo ""

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}Error: Must run from the repository root${NC}"
    exit 1
fi

# Check if git working directory is clean
if ! git diff-index --quiet HEAD --; then
    echo -e "${RED}Error: Working directory has uncommitted changes${NC}"
    echo -e "${YELLOW}Please commit or stash your changes before creating a release${NC}"
    git status --short
    exit 1
fi

# Check if on main branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${YELLOW}Warning: You're on branch '$CURRENT_BRANCH', not 'main'${NC}"
    read -p "Continue anyway? [y/N] " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep "^version" Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
echo -e "${BLUE}Current version: ${GREEN}v${CURRENT_VERSION}${NC}"
echo ""

# Ask for new version
echo -e "${YELLOW}Enter the new version number (without 'v' prefix):${NC}"
read -p "Version: " NEW_VERSION

# Validate version format (semver)
if ! [[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    echo -e "${RED}Error: Invalid version format. Use semver (e.g., 1.2.3 or 1.2.3-beta.1)${NC}"
    exit 1
fi

TAG="v${NEW_VERSION}"

# Check if tag already exists
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo -e "${RED}Error: Tag $TAG already exists${NC}"
    exit 1
fi

echo ""
echo -e "${BLUE}Release Summary:${NC}"
echo -e "  Tag:     ${GREEN}${TAG}${NC}"
echo -e "  Branch:  ${GREEN}${CURRENT_BRANCH}${NC}"
echo -e "  Commit:  ${GREEN}$(git rev-parse --short HEAD)${NC}"
echo ""
echo -e "${YELLOW}This will:${NC}"
echo "  1. Create git tag: $TAG"
echo "  2. Push tag to GitHub"
echo "  3. Trigger GitHub Actions release workflow"
echo "  4. Build binaries for:"
echo "     - Linux (x86_64, ARM64)"
echo "     - macOS (x86_64, ARM64)"
echo "     - Windows (x86_64, ARM64)"
echo "  5. Create GitHub release with binaries attached"
echo ""

read -p "Continue? [y/N] " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${YELLOW}Release cancelled${NC}"
    exit 0
fi

echo ""
echo -e "${BLUE}Creating release...${NC}"

# Create annotated tag
echo -e "${BLUE}→ Creating git tag ${TAG}...${NC}"
git tag -a "$TAG" -m "Release $TAG"

# Push tag to GitHub
echo -e "${BLUE}→ Pushing tag to GitHub...${NC}"
git push origin "$TAG"

echo ""
echo -e "${GREEN}✓ Release tag pushed successfully!${NC}"
echo ""
echo -e "${BLUE}GitHub Actions is now building release binaries.${NC}"
echo -e "${BLUE}Monitor progress at:${NC}"
echo -e "  ${YELLOW}https://github.com/$(git remote get-url origin | sed 's/.*github.com[:/]\(.*\)\.git/\1/')/actions${NC}"
echo ""
echo -e "${BLUE}The release will be available at:${NC}"
echo -e "  ${YELLOW}https://github.com/$(git remote get-url origin | sed 's/.*github.com[:/]\(.*\)\.git/\1/')/releases/tag/${TAG}${NC}"
echo ""
echo -e "${GREEN}Binaries will be uploaded for:${NC}"
echo "  • lunaroute-server-linux-amd64"
echo "  • lunaroute-server-linux-arm64"
echo "  • lunaroute-server-darwin-amd64 (macOS Intel)"
echo "  • lunaroute-server-darwin-arm64 (macOS Apple Silicon)"
echo "  • lunaroute-server-windows-amd64.exe"
echo "  • lunaroute-server-windows-arm64.exe"
echo ""
echo -e "${YELLOW}Note: The build process takes ~10-15 minutes.${NC}"
echo -e "${YELLOW}You'll receive a notification when the release is published.${NC}"
