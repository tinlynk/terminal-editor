#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
installer=$PWD/install.sh
stage=$(mktemp -d)
trap 'chmod -R u+w "$stage"; rm -rf "$stage"' EXIT
mkdir "$stage/mock" "$stage/downloads"
printf '#!/bin/sh\nexit 0\n' > "$stage/downloads/binary"
digest=$(shasum -a 256 "$stage/downloads/binary" | awk '{print $1}')
export TEST_STAGE=$stage TEST_OS=Linux TEST_ARCH=x86_64 TEST_MACOS=26.0
export TEST_TAG=v0.1.0 TEST_DOWNLOAD_FAIL='' TEST_SUDO_FAIL=''
for platform in macos-arm64 linux-x64 linux-arm64; do
    printf '%s  tinlynk-editor-v0.1.0-%s\n' "$digest" "$platform"
done > "$stage/downloads/SHA256SUMS"
cat > "$stage/mock/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "$TEST_OS" ;; -m) echo "$TEST_ARCH" ;; esac
EOF
cat > "$stage/mock/sw_vers" <<'EOF'
#!/bin/sh
echo "$TEST_MACOS"
EOF
cat > "$stage/mock/curl" <<'EOF'
#!/bin/sh
output=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift 2 ;;
        --proto|--proto-redir|--connect-timeout|--max-time|--write-out) shift 2 ;;
        --*) shift ;;
        *) url=$1; shift ;;
    esac
done
printf '%s\n' "$url" >> "$TEST_STAGE/requests"
[ -z "$TEST_DOWNLOAD_FAIL" ] || exit 22
case "$url" in
    */latest) printf 'https://github.com/tinlynk/terminal-editor/releases/tag/%s' "$TEST_TAG" ;;
    */SHA256SUMS) cp "$TEST_STAGE/downloads/SHA256SUMS" "$output" ;;
    */tinlynk-editor-*) cp "$TEST_STAGE/downloads/binary" "$output" ;;
    *) exit 22 ;;
esac
EOF
cat > "$stage/mock/sudo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$TEST_STAGE/sudo-calls"
[ -z "$TEST_SUDO_FAIL" ] || exit 1
# Simulate elevation only within the fixture directory.
chmod u+w "$TEST_STAGE/protected"
"$@"
EOF
chmod +x "$stage/mock/"*
export PATH="$stage/mock:$PATH"
export TMPDIR="$stage"

success() {
    local destination=$1
    shift
    sh "$installer" "$@" > "$stage/log" 2>&1
    cmp "$stage/downloads/binary" "$destination/tinlynk-editor"
    [[ -x "$destination/tinlynk-editor" ]]
    [[ $(find "$stage" -maxdepth 1 -name 'tinlynk-install.*' | wc -l) -eq 0 ]]
}
failure() {
    local message=$1
    shift
    if sh "$installer" "$@" > "$stage/log" 2>&1; then
        echo "Expected installer failure: $*" >&2
        exit 1
    fi
    grep -F "$message" "$stage/log" >/dev/null
    [[ $(find "$stage" -maxdepth 1 -name 'tinlynk-install.*' | wc -l) -eq 0 ]]
}
success "$stage/default latest" --bin-dir "$stage/default latest"
grep '/releases/latest' "$stage/requests" >/dev/null
grep '/download/v0.1.0/tinlynk-editor-v0.1.0-linux-x64' "$stage/requests" >/dev/null
[[ ! -e "$stage/sudo-calls" ]]
for mapping in 'Linux x86_64 linux-x64' 'Linux amd64 linux-x64' \
    'Linux aarch64 linux-arm64' 'Linux arm64 linux-arm64' \
    'Darwin arm64 macos-arm64' 'Darwin aarch64 macos-arm64'; do
    read -r TEST_OS TEST_ARCH expected <<< "$mapping"
    export TEST_OS TEST_ARCH
    : > "$stage/requests"
    success "$stage/$TEST_OS-$TEST_ARCH/bin" --version 0.1.0 \
        --prefix "$stage/$TEST_OS-$TEST_ARCH"
    grep "/download/v0.1.0/tinlynk-editor-v0.1.0-$expected" "$stage/requests" >/dev/null
    if grep '/latest' "$stage/requests" >/dev/null; then
        echo "Pinned installs must not resolve latest" >&2
        exit 1
    fi
done
export TEST_OS=Linux TEST_ARCH=x86_64
success "$stage/pinned" --version v0.1.0 --bin-dir "$stage/pinned"
chmod 0444 "$stage/pinned/tinlynk-editor"
success "$stage/pinned" --version v0.1.0 --bin-dir "$stage/pinned"
[[ ! -e "$stage/sudo-calls" ]]
success "$stage/relative/bin" --prefix "$stage/relative/"
(
    cd "$stage"
    success "$stage/relative-path/bin" --prefix relative-path
)
failure 'specify only one' --prefix "$stage/prefix" --bin-dir "$stage/bin"
failure 'requires a value' --version
failure 'requires a value' --bin-dir --version 0.1.0
failure 'unknown option' --unknown
failure 'invalid version' --version '../../escape'
failure 'invalid version' --version v01.0.0
failure 'invalid version' --version v0.1.0-rc.01
for tool in curl uname sw_vers cp awk mktemp dirname mkdir install rm rmdir shasum; do
    mkdir -p "$stage/fallback"
    ln -s "$(command -v "$tool")" "$stage/fallback/$tool"
done
PATH="$stage/fallback" /bin/sh "$installer" --version v0.1.0 \
    --bin-dir "$stage/shasum fallback" > "$stage/log" 2>&1
cmp "$stage/downloads/binary" "$stage/shasum fallback/tinlynk-editor"
rm "$stage/fallback/shasum"
if PATH="$stage/fallback" /bin/sh "$installer" --version v0.1.0 \
    --bin-dir "$stage/not-installed" > "$stage/log" 2>&1; then
    echo "Expected failure without a checksum tool" >&2
    exit 1
fi
grep 'sha256sum or shasum is required' "$stage/log" >/dev/null
rm "$stage/fallback/curl"
if PATH="$stage/fallback" /bin/sh "$installer" --version v0.1.0 \
    --bin-dir "$stage/not-installed" > "$stage/log" 2>&1; then
    echo "Expected failure without curl" >&2
    exit 1
fi
grep 'curl is required' "$stage/log" >/dev/null
[[ ! -e "$stage/not-installed" ]]
export TEST_TAG=v0.1.0-rc.1
sed 's/v0.1.0-/v0.1.0-rc.1-/g' "$stage/downloads/SHA256SUMS" > "$stage/prechecksums"
cp "$stage/downloads/SHA256SUMS" "$stage/original-checksums"
cp "$stage/prechecksums" "$stage/downloads/SHA256SUMS"
success "$stage/prerelease" --version v0.1.0-rc.1 --bin-dir "$stage/prerelease"
cp "$stage/original-checksums" "$stage/downloads/SHA256SUMS"
export TEST_TAG=not-a-version
failure 'invalid version' --bin-dir "$stage/not-installed"
export TEST_TAG=v0.1.0
export TEST_ARCH=riscv64
failure 'unsupported platform' --version 0.1.0
export TEST_OS=Darwin TEST_ARCH=x86_64
failure 'unsupported platform' --version 0.1.0
export TEST_ARCH=arm64 TEST_MACOS=15.7
failure 'macOS 26 or newer' --version 0.1.0
export TEST_OS=Linux TEST_ARCH=x86_64 TEST_DOWNLOAD_FAIL=yes
failure 'cannot download' --version 0.1.0 --bin-dir "$stage/not-installed"
failure 'cannot resolve' --bin-dir "$stage/not-installed"
[[ ! -e "$stage/not-installed" ]]
export TEST_DOWNLOAD_FAIL=
printf 'tampered\n' >> "$stage/downloads/binary"
failure 'checksum mismatch' --version 0.1.0 --bin-dir "$stage/not-installed"
[[ ! -e "$stage/not-installed" ]]
printf '#!/bin/sh\nexit 0\n' > "$stage/downloads/binary"
cat "$stage/original-checksums" "$stage/original-checksums" > "$stage/downloads/SHA256SUMS"
failure 'duplicate' --version 0.1.0 --bin-dir "$stage/not-installed"
printf '%s  another-asset\n' "$digest" > "$stage/downloads/SHA256SUMS"
failure 'missing' --version 0.1.0 --bin-dir "$stage/not-installed"
cp "$stage/original-checksums" "$stage/downloads/SHA256SUMS"
mkdir "$stage/link"
ln -s "$stage/downloads/binary" "$stage/link/tinlynk-editor"
failure 'symlink' --version 0.1.0 --bin-dir "$stage/link"
mkdir -p "$stage/directory/tinlynk-editor"
failure 'executable is a directory' --version 0.1.0 --bin-dir "$stage/directory"
touch "$stage/file-ancestor"
failure 'ancestor is not a directory' --version 0.1.0 --bin-dir "$stage/file-ancestor/bin"
mkdir "$stage/protected"
chmod 0555 "$stage/protected"
if [[ ! -w "$stage/protected" ]]; then
    export TEST_SUDO_FAIL=yes
    failure 'with sudo' --version 0.1.0 --bin-dir "$stage/protected"
    [[ ! -e "$stage/protected/tinlynk-editor" ]]
    export TEST_SUDO_FAIL=
    : > "$stage/sudo-calls"
    success "$stage/protected" --version 0.1.0 --bin-dir "$stage/protected"
    [[ $(wc -l < "$stage/sudo-calls") -eq 2 ]]
else
    echo "Privilege simulation skipped: current user can write mode-0555 directories"
fi
sh "$installer" --help | grep 'Usage:' >/dev/null
echo "Installer tests passed"
