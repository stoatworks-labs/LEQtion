#!/usr/bin/env bash
# release-local.sh — cut a LEQtion release from this Mac.
#
# GitHub Actions minutes are exhausted, so releases are built here. The fleet's
# shared pipeline (`release-rust.sh`) does not fit this repo: it builds a Rust
# *server* binary with an optional Tauri launcher beside it, and LEQtion has no
# server — the Tauri app is the whole product. So this script drives
# `tauri build` directly and uses the shared helpers only for staging and
# signing.
#
#   scripts/release-local.sh                  build into dist-release/
#   scripts/release-local.sh --version 0.2.0  set an explicit version
#   scripts/release-local.sh --upload         tag and publish the GitHub release
#
# ## macOS only, and that is a statement about the builds, not the code
#
# Tauri cannot cross-bundle: an .app, an .msi and an .AppImage each have to be
# produced on their own OS. This host is an arm64 Mac, so it can make both macOS
# architectures and nothing else.
#
#   * **Windows** would need the Parallels VM, which is **ARM64** Windows — so it
#     could only ever produce an arm64 installer, which is not what a Windows user
#     with an audio interface is running. Shipping an arm64-only Windows build
#     under a bare "Windows" label would be worse than shipping none.
#   * **Linux** needs a Linux host for webkit2gtk. There isn't one.
#
# Both are absences of a *build*, not of support: the code compiles for them.
# The README says which platforms have binaries; do not let that drift.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"
source "$repo/scripts/release-lib.sh"

NAME="LEQtion"
SLUG="leqtion"
IDENT="com.allansargeant.leqtion"

out="$repo/dist-release"
upload=0
version=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --upload)  upload=1 ;;
    --version) version="$2"; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

current="$(node -p "require('$repo/package.json').version")"
version="${version:-$current}"
rl_init "$NAME" "$SLUG" "$version" "$IDENT" "$out"

# ------------------------------------------------------------- versioning ---
# Three files carry the version and they must agree: package.json is what the
# build reads, Cargo.toml is what the binary reports, and tauri.conf.json is
# what the bundle's Info.plist ends up with. A mismatch shows up as an installer
# whose "about" box disagrees with its own filename.
bump() { # bump <file> <match> <replacement>
  local file="$1" match="$2" repl="$3"
  [[ -f "$file" ]] || return 0
  perl -0pi -e "s/\Q$match\E/$repl/" "$file"
}
if [[ "$version" != "$current" ]]; then
  rl_step "version $current -> $version"
  bump package.json                "\"version\": \"$current\"" "\"version\": \"$version\""
  bump src-tauri/tauri.conf.json   "\"version\": \"$current\"" "\"version\": \"$version\""
  bump src-tauri/Cargo.toml        "version = \"$current\""    "version = \"$version\""
fi

# --------------------------------------------------------------- preflight ---
# Run the tests before building, not after. A release that has to be re-cut
# because the suite was red is a tag burnt for nothing, and this repo's whole
# claim rests on those numbers.
rl_step "tests"
npm test --silent >/dev/null
( cd src-tauri && cargo test --workspace --quiet >/dev/null )
rl_note "frontend and workspace green"

rl_step "npm install"
npm install --silent --no-audit --no-fund >/dev/null

# ------------------------------------------------------------------ builds ---
rm -rf "$out"; mkdir -p "$out"

build_mac() { # build_mac <label> <rust-target>
  local label="$1" target="$2"
  if ! rustup target list --installed | grep -qx "$target"; then
    rl_skip "$label (rust target not installed: $target)"
    return 0
  fi

  rl_step "build $label"
  npm run tauri -- build --target "$target" --bundles app,dmg >/dev/null

  local bundle="src-tauri/target/$target/release/bundle"
  local app="$bundle/macos/$NAME.app"
  local dmg
  dmg="$(find "$bundle/dmg" -name '*.dmg' -maxdepth 1 2>/dev/null | head -1)"

  if [[ ! -d "$app" ]]; then
    rl_skip "$label (no .app was produced)"
    return 0
  fi

  # Developer ID-signs and notarises when this Mac is configured for it,
  # ad-hoc otherwise. Must happen before the app is copied anywhere: the
  # notarisation ticket is stapled into the bundle, and only copies made
  # afterwards carry it.
  rl_adhoc_sign "$app"

  local stage="$out/.stage-$label"
  rm -rf "$stage"; mkdir -p "$stage"
  cp -R "$app" "$stage/"
  cp README.md LICENSE "$stage/" 2>/dev/null || true
  rl_zip "$label" "$stage"
  rm -rf "$stage"

  if [[ -n "$dmg" ]]; then
    local out_dmg="$out/${SLUG}-${version}-${label}.dmg"
    cp "$dmg" "$out_dmg"
    # Tauri built this image from the app BEFORE it was notarised, so the image
    # carries neither its own ticket nor the app's staple. Gatekeeper checks the
    # .dmg the user downloaded first, so shipping it un-notarised reinstates the
    # very warning this whole exercise removes — v0.1.1 went out that way.
    # Rebuild it from the stapled app, then notarise and staple the image.
    if rl_mac_sign_ready; then
      local dstage="$out/.dmgstage-$label"
      rm -rf "$dstage"; mkdir -p "$dstage"
      cp -R "$app" "$dstage/"
      rm -f "$out_dmg"
      rl_dmg "$label" "$dstage" --app "$NAME.app"
      rm -rf "$dstage"
      rl_mac_notarize "$out_dmg" || return 1
    fi
    rl_note "$(basename "$out_dmg")"
  else
    rl_skip "$label dmg (bundler produced none)"
  fi
}

build_mac macos-aarch64 aarch64-apple-darwin
build_mac macos-x86_64  x86_64-apple-darwin

rl_skip "windows (needs an x86_64 Windows host; the VM here is arm64)"
rl_skip "linux (needs a Linux host for webkit2gtk)"

rl_step "artefacts"
ls -1 "$out" | sed 's/^/    /'

# ------------------------------------------------------------------ upload ---
if [[ "$upload" -eq 1 ]]; then
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "working tree is dirty — commit the version bump first" >&2
    exit 1
  fi
  rl_step "tag v$version"
  git tag -a "v$version" -m "$NAME v$version"
  git push origin "v$version"

  rl_step "github release"
  gh release create "v$version" "$out"/*.dmg "$out"/*.zip \
    --title "$NAME v$version" \
    --notes-file "$repo/docs/release-notes-v$version.md"
  rl_note "https://github.com/stoatworks-labs/$NAME/releases/tag/v$version"
fi

rl_step "done"
