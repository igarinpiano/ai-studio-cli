#!/bin/sh
# Install aistudio (Google AI Studio CLI) from GitHub Releases. macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.sh | sh
#
# Environment variables:
#   AISTUDIO_VERSION      Release tag to install, e.g. v0.1.0 (default: latest)
#   AISTUDIO_INSTALL_DIR  Where to put the binary (default: ~/.local/bin)
#   GITHUB_TOKEN / GH_TOKEN  Token for a private repository (also taken from
#                         `gh auth token` when the GitHub CLI is logged in)
#   AISTUDIO_GITHUB_API / AISTUDIO_GITHUB_URL  API and web base URLs
#                         (default: https://api.github.com, https://github.com)

set -eu

REPO="igarinpiano/ai-studio-cli"
BIN="aistudio"
VERSION="${AISTUDIO_VERSION:-latest}"
INSTALL_DIR="${AISTUDIO_INSTALL_DIR:-$HOME/.local/bin}"
API="${AISTUDIO_GITHUB_API:-https://api.github.com}"
WEB="${AISTUDIO_GITHUB_URL:-https://github.com}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        arm64 | aarch64) arch=aarch64 ;;
        *) err "unsupported CPU architecture: $arch" ;;
    esac
    case "$os" in
        Darwin)
            # An x86_64 shell under Rosetta on Apple silicon: prefer the native build.
            if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
                arch=aarch64
            fi
            echo "${arch}-apple-darwin"
            ;;
        # Static musl builds run on any distribution.
        Linux) echo "${arch}-unknown-linux-musl" ;;
        MINGW* | MSYS* | CYGWIN*) err "on Windows, use install.ps1 (see README)" ;;
        *) err "unsupported OS: $os" ;;
    esac
}

# http_get URL OUTPUT ACCEPT — with the token when there is one.
http_get() {
    if has curl; then
        if [ -n "$TOKEN" ]; then
            curl -fsSL --retry 3 -H "Accept: $3" -H "Authorization: Bearer $TOKEN" -o "$2" "$1"
        else
            curl -fsSL --retry 3 -H "Accept: $3" -o "$2" "$1"
        fi
    elif has wget; then
        if [ -n "$TOKEN" ]; then
            wget -q --header="Accept: $3" --header="Authorization: Bearer $TOKEN" -O "$2" "$1"
        else
            wget -q --header="Accept: $3" -O "$2" "$1"
        fi
    else
        err "curl or wget is required"
    fi
}

# API URL of a release asset, read from the release JSON.
asset_api_url() {
    tr ',{}[]' '\n' <"$tmp/release.json" | awk -v want="$1" '
        /"url": *"[^"]*\/releases\/assets\/[0-9]+"/ { u = $0; sub(/.*"url": *"/, "", u); sub(/".*/, "", u) }
        /"name": *"/ { n = $0; sub(/.*"name": *"/, "", n); sub(/".*/, "", n); if (n == want && u != "") { print u; exit } }'
}

# fetch ASSET OUTPUT
fetch() {
    if [ -n "$TOKEN" ]; then
        # Private repositories serve assets only through the API. (curl drops
        # the token on the redirect to the storage host; wget would not.)
        has curl || err "curl is required to download with a token"
        url=$(asset_api_url "$1")
        [ -n "$url" ] || err "release ${tag} has no asset $1"
        http_get "$url" "$2" application/octet-stream
    else
        http_get "${WEB}/${REPO}/releases/download/${tag}/$1" "$2" application/octet-stream
    fi
}

sha256_of() {
    if has sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

main() {
    if [ -z "$TOKEN" ] && has gh; then
        TOKEN=$(gh auth token 2>/dev/null || true)
    fi
    target=$(detect_target)
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    if [ "$VERSION" = latest ]; then
        release_url="${API}/repos/${REPO}/releases/latest"
    else
        release_url="${API}/repos/${REPO}/releases/tags/${VERSION}"
    fi
    if ! http_get "$release_url" "$tmp/release.json" application/vnd.github+json; then
        hint=""
        [ -z "$TOKEN" ] && hint=" (for a private repository, set GITHUB_TOKEN or run 'gh auth login')"
        err "could not find release '${VERSION}' of ${REPO}${hint}"
    fi
    tag=$(tr ',' '\n' <"$tmp/release.json" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$tag" ] || err "unexpected response from ${release_url}"
    version=${tag#v}
    name="${BIN}-${version}-${target}"
    asset="${name}.tar.gz"

    say "Downloading ${asset}..."
    fetch "$asset" "$tmp/$asset"

    if fetch SHA256SUMS "$tmp/SHA256SUMS" 2>/dev/null; then
        expected=$(awk -v n="$asset" '$2 == n || $2 == "*" n { print $1 }' "$tmp/SHA256SUMS")
        actual=$(sha256_of "$tmp/$asset")
        if [ -z "$expected" ]; then
            say "warning: ${asset} is not listed in SHA256SUMS; skipping verification"
        elif [ -z "$actual" ]; then
            say "warning: no sha256 tool found; skipping verification"
        elif [ "$expected" != "$actual" ]; then
            err "checksum mismatch for ${asset} (expected ${expected}, got ${actual})"
        fi
    else
        say "warning: no SHA256SUMS in release ${tag}; skipping verification"
    fi

    tar -xzf "$tmp/$asset" -C "$tmp"
    [ -f "$tmp/$name/$BIN" ] || err "archive did not contain ${name}/${BIN}"

    mkdir -p "$INSTALL_DIR"
    if [ -w "$INSTALL_DIR" ]; then
        install -m 755 "$tmp/$name/$BIN" "$INSTALL_DIR/$BIN"
    else
        say "Installing to ${INSTALL_DIR} needs sudo."
        sudo install -m 755 "$tmp/$name/$BIN" "$INSTALL_DIR/$BIN"
    fi

    say "Installed $("$INSTALL_DIR/$BIN" --version 2>/dev/null || echo "$BIN $tag") to ${INSTALL_DIR}/${BIN}"

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            say ""
            say "${INSTALL_DIR} is not on your PATH. Add it, e.g.:"
            case "${SHELL:-}" in
                */fish) say "  fish_add_path ${INSTALL_DIR}" ;;
                */zsh) say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> \"\$HOME/.zshrc\"" ;;
                */bash) say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> \"\$HOME/.bashrc\"" ;;
                *) say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> \"\$HOME/.profile\"" ;;
            esac
            ;;
    esac
    say ""
    say "Next: get an API key at https://aistudio.google.com/apikey, then run: ${BIN} login"
}

main "$@"
