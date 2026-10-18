#!/bin/sh
# Aether installer: downloads a released binary, checks its SHA-256 and
# installs it without sudo. Options and manual steps: docs/install.md.
#
#   curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | sh
#
# Environment:
#   AETHER_VERSION           release tag to install, e.g. v1.0.0 (default: latest)
#   AETHER_INSTALL_DIR       where the binary goes (default: $HOME/.local/bin)
#   AETHER_INSTALL_BASE_URL  download base for mirrors and tests
#                            (default: https://github.com/mengsig/aether/releases)
set -eu

DEFAULT_BASE_URL="https://github.com/mengsig/aether/releases"
BINARY="aether"
RECEIPT=".aether-receipt"
WORK_DIR=""
STAGED_FILE=""

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'install.sh: error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$STAGED_FILE" ]; then
    rm -f "$STAGED_FILE"
  fi
  if [ -n "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
}

have() {
  command -v "$1" >/dev/null 2>&1
}

# Prints the release target for this machine, or refuses with the reason.
detect_target() {
  os=$(uname -s)
  cpu=$(uname -m)
  case "$os" in
    Linux) detect_linux_target "$cpu" ;;
    Darwin) detect_macos_target "$cpu" ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT)
      fail "Windows is not supported natively; use WSL 2 and run this installer inside it"
      ;;
    *)
      fail "unsupported operating system '$os'; Aether is released for Linux and macOS (building from source: docs/install.md)"
      ;;
  esac
}

detect_linux_target() {
  case "$1" in
    x86_64 | amd64) say "x86_64-unknown-linux-musl" ;;
    aarch64 | arm64) say "aarch64-unknown-linux-musl" ;;
    *) fail "unsupported CPU '$1' on Linux; releases exist for x86_64 and aarch64 (building from source: docs/install.md)" ;;
  esac
}

detect_macos_target() {
  cpu=$1
  # A shell translated by Rosetta reports x86_64 on Apple silicon.
  if [ "$cpu" = "x86_64" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || say 0)" = "1" ]; then
    cpu="arm64"
  fi
  case "$cpu" in
    arm64 | aarch64) say "aarch64-apple-darwin" ;;
    x86_64) say "x86_64-apple-darwin" ;;
    *) fail "unsupported CPU '$cpu' on macOS; releases exist for arm64 and x86_64" ;;
  esac
}

# Prints the download base without a trailing slash. Plain http is accepted
# for this machine only: a checksum fetched over it proves nothing elsewhere.
resolve_base_url() {
  base=${AETHER_INSTALL_BASE_URL:-$DEFAULT_BASE_URL}
  base=${base%/}
  case "$base" in
    *@*) fail "AETHER_INSTALL_BASE_URL must not contain userinfo (@)" ;;
  esac
  case "$base" in
    https://?*) ;;
    http://127.0.0.1 | http://127.0.0.1[:/]* | http://localhost | http://localhost[:/]*) ;;
    http://\[::1\] | http://\[::1\][:/]*) ;;
    *) fail "AETHER_INSTALL_BASE_URL must be an https:// URL (http:// only for 127.0.0.1, localhost or [::1]), got '$base'" ;;
  esac
  say "$base"
}

# Prints the URL directory that holds the assets of the wanted release.
resolve_release_url() {
  base=$1
  version=${AETHER_VERSION:-}
  if [ -z "$version" ] || [ "$version" = "latest" ]; then
    say "$base/latest/download"
    return
  fi
  case "$version" in
    v*) ;;
    *) version="v$version" ;;
  esac
  if ! say "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
    fail "AETHER_VERSION must look like v1.0.0, got '${AETHER_VERSION}'"
  fi
  say "$base/download/$version"
}

MAX_REDIRECTS=5

# Prints an absolute path (starting with /) without its . and .. segments,
# as wget sends it, so later relative redirects resolve against the real URL.
remove_dot_segments() {
  rest=${1#/}
  output=""
  while :; do
    segment=${rest%%/*}
    case "$segment" in
      .) trailing=/ ;;
      ..) output=${output%/*}; trailing=/ ;;
      *) output="$output/$segment"; trailing="" ;;
    esac
    case "$rest" in
      */*) rest=${rest#*/} ;;
      *) break ;;
    esac
  done
  say "$output$trailing"
}

# Prints an https URL with dot segments removed from its path.
normalize_url() {
  rest=${1#https://}
  authority_path=${rest%%[?#]*}
  suffix=${rest#"$authority_path"}
  host=${authority_path%%/*}
  case "$authority_path" in
    */*) path=$(remove_dot_segments "/${authority_path#*/}") ;;
    *) path="" ;;
  esac
  say "https://$host$path$suffix"
}

# Prints the https URL a `wget -S` header dump redirects to, or nothing when
# there is no redirect. Fails on a hop that is not https. Arguments: current URL, header file.
redirect_target() {
  current=$1
  headers=$2
  location=$(sed -n 's/^[[:space:]]*[Ll][Oo][Cc][Aa][Tt][Ii][Oo][Nn]:[[:space:]]*//p' "$headers" |
    tail -n 1 | tr -d '\r')
  location=${location% \[following\]}
  [ -n "$location" ] || return 0
  case "$current" in
    https://?*) ;;
    *) fail "could not download $current: only https URLs may be redirected" ;;
  esac
  case "${location%%/*}" in
    *:)
      case "$location" in
        https://?*) normalize_url "$location"; return ;;
      esac
      fail "could not download $current: refused redirect to non-https URL '$location'"
      ;;
  esac
  withoutfragment=${current%%\#*}
  rest=${withoutfragment#https://}
  hostpath=${rest%%\?*}
  host=${hostpath%%/*}
  path=/
  case "$hostpath" in */*) path=/${hostpath#*/} ;; esac
  case "$location" in
    //?*) normalize_url "https:$location" ;;
    /*) normalize_url "https://$host$location" ;;
    \?*) normalize_url "https://$host$path$location" ;;
    \#*) normalize_url "$withoutfragment$location" ;;
    *) normalize_url "https://$host${path%/*}/$location" ;;
  esac
}

# wget cannot follow redirects only while they stay https, so each hop is
# fetched with redirects off and its Location is checked here.
wget_download() {
  url=$1
  dest=$2
  case "$url" in
    http://*)
      wget --max-redirect=0 -q -T 60 -O "$dest" "$url" ||
        fail "could not download $url; wget redirects are disabled for safety, install curl for HTTPS redirects"
      return
      ;;
  esac
  wget --help 2>&1 | grep -q -- '--max-redirect' ||
    fail "this wget cannot limit redirects (busybox?); install curl"
  headers="$dest.headers"
  hops=0
  while ! wget --https-only --max-redirect=0 -S -T 60 -O "$dest" "$url" 2>"$headers"; do
    next=$(redirect_target "$url" "$headers") || exit 1
    [ -n "$next" ] || fail "could not download $url; check the version and your network"
    hops=$((hops + 1))
    [ "$hops" -le "$MAX_REDIRECTS" ] ||
      fail "could not download $url: more than $MAX_REDIRECTS redirects"
    url=$next
  done
  rm -f "$headers"
}

download() {
  url=$1
  dest=$2
  if have curl; then
    protocols='=https'
    case "$url" in http://*) protocols='=http,https' ;; esac
    curl --proto "$protocols" --proto-redir '=https' \
      --fail --silent --show-error --location --retry 2 \
      --connect-timeout 15 --max-time 600 --output "$dest" "$url" ||
      fail "could not download $url; check the version and your network"
  elif have wget; then
    wget_download "$url" "$dest"
  else
    fail "curl or wget is needed to download Aether"
  fi
  [ -s "$dest" ] || fail "downloaded $url but the file is empty"
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    fail "sha256sum or shasum is needed to verify the download; refusing to install an unverified binary"
  fi
}

verify_checksum() {
  archive=$1
  checksum_file=$2
  expected=$(cut -d ' ' -f 1 <"$checksum_file" | head -n 1)
  if ! say "$expected" | grep -Eq '^[0-9a-fA-F]{64}$'; then
    fail "the published checksum is not a SHA-256 digest; refusing to install"
  fi
  actual=$(sha256_of "$archive")
  expected=$(say "$expected" | tr 'A-F' 'a-f')
  if [ "$actual" != "$expected" ]; then
    fail "checksum mismatch for $(basename "$archive"): expected $expected, got $actual; nothing was installed"
  fi
}

# Unpacks the binary and moves it into place with one rename, so a running
# or concurrent `aether` never sees a half-written file.
install_binary() {
  archive=$1
  install_dir=$2
  mkdir "$WORK_DIR/unpack"
  tar -xzf "$archive" -C "$WORK_DIR/unpack" "$BINARY" ||
    fail "could not unpack $BINARY from $(basename "$archive")"
  unpacked="$WORK_DIR/unpack/$BINARY"
  if [ ! -f "$unpacked" ] || [ -L "$unpacked" ]; then
    fail "the archive does not hold a regular file named $BINARY"
  fi
  mkdir -p "$install_dir" || fail "could not create $install_dir"
  if [ -d "$install_dir/$BINARY" ]; then
    fail "$install_dir/$BINARY is a directory; remove it or set AETHER_INSTALL_DIR"
  fi
  STAGED_FILE="$install_dir/.$BINARY.install.$$"
  cp "$unpacked" "$STAGED_FILE" ||
    fail "could not write to $install_dir; set AETHER_INSTALL_DIR to a directory you own"
  chmod 755 "$STAGED_FILE"
  mv -f "$STAGED_FILE" "$install_dir/$BINARY"
  STAGED_FILE=""
  write_receipt "$install_dir"
}

# `aether update` replaces only a binary that has this receipt beside it.
write_receipt() {
  install_dir=$1
  STAGED_FILE="$install_dir/$RECEIPT.$$"
  {
    say "# Written by Aether's install.sh; safe to delete together with $BINARY."
    say "installer=install.sh"
  } >"$STAGED_FILE"
  mv -f "$STAGED_FILE" "$install_dir/$RECEIPT"
  STAGED_FILE=""
}

report() {
  install_dir=$1
  installed=$("$install_dir/$BINARY" --version) ||
    fail "$install_dir/$BINARY was installed but does not run on this machine"
  say "Installed $installed to $install_dir/$BINARY"
  case ":${PATH:-}:" in
    *":$install_dir:"*)
      first=$(command -v "$BINARY" || true)
      if [ -n "$first" ] && [ "$first" != "$install_dir/$BINARY" ]; then
        say "Note: another $BINARY at $first comes first on your PATH."
      fi
      ;;
    *)
      say "$install_dir is not on your PATH. Add this line to your shell profile:"
      say "  export PATH=\"$install_dir:\$PATH\""
      ;;
  esac
  say "Next: run \`$BINARY\` in a project directory. Update later with \`$BINARY update\`."
}

main() {
  [ "$#" -eq 0 ] || fail "this installer takes no arguments; it is configured through AETHER_VERSION, AETHER_INSTALL_DIR and AETHER_INSTALL_BASE_URL"
  [ -n "${HOME:-}" ] || [ -n "${AETHER_INSTALL_DIR:-}" ] || fail "HOME is not set; set AETHER_INSTALL_DIR"
  install_dir=${AETHER_INSTALL_DIR:-$HOME/.local/bin}
  case "$install_dir" in
    /*) ;;
    *) fail "AETHER_INSTALL_DIR must be an absolute path, got '$install_dir'" ;;
  esac
  install_dir=${install_dir%/}
  have tar || fail "tar is needed to unpack Aether"

  target=$(detect_target)
  base=$(resolve_base_url)
  release_url=$(resolve_release_url "$base")
  asset="$BINARY-$target.tar.gz"

  WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aether-install.XXXXXX")
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM HUP

  say "Downloading $release_url/$asset"
  download "$release_url/$asset" "$WORK_DIR/$asset"
  download "$release_url/$asset.sha256" "$WORK_DIR/$asset.sha256"
  verify_checksum "$WORK_DIR/$asset" "$WORK_DIR/$asset.sha256"
  install_binary "$WORK_DIR/$asset" "$install_dir"
  report "$install_dir"
}

main "$@"
