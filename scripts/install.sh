#!/bin/sh
# Installs `sion`, into <prefix>/bin (by default ~/.local/bin).
#
#   sh scripts/install.sh                         from the checkout it is in
#   sh scripts/install.sh --git [--rev <ref>]     cargo install from the repository
#   sh scripts/install.sh --release <base URL>    a release archive, checked against
#                                                 its SHA256SUMS (and their signature,
#                                                 with --allowed-signers <file>)
#
# Options: --prefix <dir>, --dry-run (say what it would do, and do nothing).
# Building needs Rust (https://rustup.rs) and a C compiler (for aws-lc, the
# cryptography under TLS). It never uses root: a prefix it cannot write is
# refused.
set -eu

REPO="https://github.com/ColdAI-org/siontology"
SIGNER="releases@siontology.org"
prefix="${HOME}/.local"
mode=""
source_dir=""
release=""
rev=""
signers=""
dry=0

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
run() {
  say "+ $*"
  if [ "$dry" -eq 0 ]; then "$@"; fi
}
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is needed: $2"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
    --source) mode=source; source_dir="${2:?--source needs a directory}"; shift 2 ;;
    --git) mode=git; shift ;;
    --rev) rev="${2:?--rev needs a ref}"; shift 2 ;;
    --release) mode=release; release="${2:?--release needs a base URL}"; shift 2 ;;
    --allowed-signers) signers="${2:?--allowed-signers needs a file}"; shift 2 ;;
    --dry-run) dry=1; shift ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) fail "unknown option $1 (see --help)" ;;
  esac
done

if [ -z "$mode" ]; then
  here=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd || true)
  if [ -n "$here" ] && [ -f "$here/crates/sion-cli/Cargo.toml" ]; then
    mode=source
    source_dir="$here"
  else
    mode=git
  fi
fi

if [ "$dry" -eq 0 ]; then
  mkdir -p "$prefix/bin" 2>/dev/null || true
  [ -w "$prefix/bin" ] || fail "$prefix/bin cannot be written: choose another --prefix, or make it yours"
fi

builds() {
  need cargo "install Rust from https://rustup.rs"
  need cc "a C compiler builds aws-lc (apt install build-essential; on macOS, xcode-select --install)"
}

target() {
  case "$(uname -s)" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) fail "no release is made for $(uname -s): build from source (--git)" ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) fail "no release is made for $(uname -m): build from source (--git)" ;;
  esac
  printf '%s-%s' "$arch" "$os"
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

case "$mode" in
  source)
    builds
    say "building sion from $source_dir"
    run cargo install --locked --path "$source_dir/crates/sion-cli" --root "$prefix"
    ;;
  git)
    builds
    say "building sion from $REPO${rev:+ at $rev}"
    if [ -n "$rev" ]; then
      run cargo install --locked --git "$REPO" --rev "$rev" --root "$prefix" sion-cli
    else
      run cargo install --locked --git "$REPO" --root "$prefix" sion-cli
    fi
    ;;
  release)
    need curl "to fetch the release"
    need tar "to unpack it"
    archive="sion-$(target).tar.gz"
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT INT TERM
    run curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$archive" "$release/$archive"
    run curl -fsSL --proto '=https' --tlsv1.2 -o "$work/SHA256SUMS" "$release/SHA256SUMS"
    if [ -n "$signers" ]; then
      need ssh-keygen "to check the signature"
      run curl -fsSL --proto '=https' --tlsv1.2 -o "$work/SHA256SUMS.sig" "$release/SHA256SUMS.sig"
    fi
    if [ "$dry" -eq 0 ]; then
      if [ -n "$signers" ]; then
        ssh-keygen -Y verify -f "$signers" -I "$SIGNER" -n file -s "$work/SHA256SUMS.sig" \
          < "$work/SHA256SUMS" >/dev/null 2>&1 \
          || fail "SHA256SUMS is not signed by $SIGNER in $signers: not installed"
        say "SHA256SUMS is signed by $SIGNER"
      fi
      expected=$(grep " $archive\$" "$work/SHA256SUMS" | cut -d' ' -f1)
      [ -n "$expected" ] || fail "SHA256SUMS names no $archive"
      [ "$(sha256 "$work/$archive")" = "$expected" ] \
        || fail "$archive does not match its SHA-256 in SHA256SUMS: not installed"
      say "$archive matches its SHA-256"
      tar -xzf "$work/$archive" -C "$work"
      [ -f "$work/sion" ] || fail "$archive holds no sion"
      cp "$work/sion" "$prefix/bin/sion.new"
      chmod 0755 "$prefix/bin/sion.new"
      mv "$prefix/bin/sion.new" "$prefix/bin/sion"
    fi
    ;;
esac

case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) say "$prefix/bin is not on your PATH: add it, e.g. export PATH=\"$prefix/bin:\$PATH\"" ;;
esac
if [ "$dry" -eq 1 ]; then
  say "(a dry run: nothing was installed)"
else
  say "installed $prefix/bin/sion; \`sion doctor\` says what else is here"
fi
