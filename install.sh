#!/bin/sh
set -eu

die() {
    printf 'tinlynk installer: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: sh install.sh [--version VERSION] [--prefix PREFIX | --bin-dir DIRECTORY]

Install tinlynk-editor from a GitHub Release.
  --version VERSION    Pin a release (e.g. v0.1.0 or 0.1.0); default: latest stable
  --prefix PREFIX      Install into PREFIX/bin
  --bin-dir DIRECTORY  Install into this exact directory; default: /usr/local/bin
  --help               Show this help

Requires curl and sha256sum or shasum. Linux requires glibc 2.35+.
macOS requires version 26+ on Apple Silicon.
EOF
}

version=latest
bin_dir=/usr/local/bin
destination_set=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --help) usage; exit 0 ;;
        --version|--prefix|--bin-dir)
            option=$1
            [ "$#" -ge 2 ] && [ -n "$2" ] || die "$option requires a value"
            case "$2" in --*) die "$option requires a value" ;; esac
            case "$option" in
                --version) version=$2 ;;
                *)
                    [ "$destination_set" = false ] || die "specify only one --prefix or --bin-dir"
                    destination_set=true
                    case "$option" in
                        --prefix) bin_dir=${2%/}/bin ;;
                        --bin-dir) bin_dir=$2 ;;
                    esac
                    ;;
            esac
            shift 2
            ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

case "$bin_dir" in
    /*) ;;
    *) bin_dir=$PWD/$bin_dir ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required"
if command -v sha256sum >/dev/null 2>&1; then
    checksum_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    checksum_tool=shasum
else
    die "sha256sum or shasum is required"
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Darwin:arm64|Darwin:aarch64)
        platform=macos-arm64
        macos_version=$(sw_vers -productVersion) || die "cannot determine macOS version"
        macos_major=${macos_version%%.*}
        case "$macos_major" in ''|*[!0-9]*) die "invalid macOS version: $macos_version" ;; esac
        [ "$macos_major" -ge 26 ] || die "macOS 26 or newer is required"
        ;;
    Linux:x86_64|Linux:amd64) platform=linux-x64 ;;
    Linux:aarch64|Linux:arm64) platform=linux-arm64 ;;
    *) die "unsupported platform: $os $arch" ;;
esac

release_base=https://github.com/tinlynk/terminal-editor/releases
if [ "$version" = latest ]; then
    release_url=$(curl --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 60 \
        --output /dev/null --write-out '%{url_effective}' "$release_base/latest") \
        || die "cannot resolve the latest release"
    case "$release_url" in
        "$release_base/tag/"*) version=${release_url##*/} ;;
        *) die "latest release did not resolve to a release tag" ;;
    esac
fi
version=${version#v}
printf '%s\n' "$version" | awk '
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/ { exit 1 }
    {
        sub(/^[0-9]+\.[0-9]+\.[0-9]+-?/, "")
        n = split($0, parts, ".")
        for (i = 1; i <= n; i++)
            if (parts[i] ~ /^0[0-9]+$/) exit 1
    }
' || die "invalid version (expected 0.1.0 or 0.1.0-rc.1)"
tag=v$version
asset=tinlynk-editor-$tag-$platform

umask 077
temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/tinlynk-install.XXXXXXXX") \
    || die "cannot create temporary download directory"
trap 'rm -f "$temp_dir/binary" "$temp_dir/SHA256SUMS"; rmdir "$temp_dir"' EXIT
trap 'exit 1' HUP INT TERM

download() {
    curl --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 300 \
        --output "$2" "$release_base/download/$tag/$1" \
        || die "cannot download $1 from release $tag"
}
download "$asset" "$temp_dir/binary"
download SHA256SUMS "$temp_dir/SHA256SUMS"
expected=$(awk -v asset="$asset" '
    $2 == asset { count++; digest = $1 }
    END {
        if (count != 1 || length(digest) != 64 || digest ~ /[^0-9a-f]/) exit 1
        print digest
    }
' "$temp_dir/SHA256SUMS") || die "missing, duplicate, or invalid checksum for $asset"
if [ "$checksum_tool" = sha256sum ]; then
    actual=$(sha256sum "$temp_dir/binary") || die "cannot compute checksum"
else
    actual=$(shasum -a 256 "$temp_dir/binary") || die "cannot compute checksum"
fi
actual=${actual%% *}
[ "$actual" = "$expected" ] || die "checksum mismatch for $asset; nothing installed"

umask 022
[ ! -d "$bin_dir/tinlynk-editor" ] || die "destination executable is a directory"
[ ! -L "$bin_dir/tinlynk-editor" ] || die "destination executable is a symlink; remove it explicitly first"
# A writable ancestor permits creating a user-owned destination without sudo.
ancestor=$bin_dir
while [ ! -e "$ancestor" ]; do
    ancestor=$(dirname "$ancestor")
done
[ -d "$ancestor" ] || die "destination ancestor is not a directory: $ancestor"
if [ -w "$ancestor" ]; then
    mkdir -p "$bin_dir" || die "cannot create $bin_dir"
    install -m 0755 "$temp_dir/binary" "$bin_dir/tinlynk-editor" \
        || die "cannot install into $bin_dir"
else
    command -v sudo >/dev/null 2>&1 \
        || die "permission denied; choose --prefix \"\$HOME/.local\" or install sudo"
    printf 'Installation into %s requires administrator permission.\n' "$bin_dir"
    sudo mkdir -p "$bin_dir" || die "cannot create $bin_dir with sudo"
    sudo install -m 0755 "$temp_dir/binary" "$bin_dir/tinlynk-editor" \
        || die "cannot install into $bin_dir with sudo"
fi
printf 'Installed tinlynk-editor %s into %s\n' "$tag" "$bin_dir/tinlynk-editor"
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) printf 'Add this directory to your PATH: %s\n' "$bin_dir" ;;
esac
