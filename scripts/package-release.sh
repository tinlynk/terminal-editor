#!/bin/bash
set -euo pipefail

if [[ $# != 3 ]]; then
    echo "Usage: bash scripts/package-release.sh VERSION TARGET OUTPUT_DIRECTORY" >&2
    exit 1
fi
tag=v${1#v}
target=$2
mkdir -p "$3"
output=$(cd "$3" && pwd)
cd "$(dirname "$0")/.."
metadata=$(cargo metadata --no-deps --format-version 1 --locked)
version=$(jq -r '.packages[] | select(.name == "tinlynk-terminal-editor") | .version' <<< "$metadata")
[[ "$tag" == "v$version" ]] || { echo "Tag $tag does not match Cargo version v$version" >&2; exit 1; }
case "$version" in
    *+*) echo "Release versions with build metadata are not supported" >&2; exit 1 ;;
esac
case "$target" in
    aarch64-apple-darwin)
        [[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] \
            || { echo "Build macOS artifacts on an ARM64 Mac" >&2; exit 1; }
        macos_version=$(sw_vers -productVersion)
        [[ ${macos_version%%.*} -ge 26 ]] \
            || { echo "Build macOS artifacts on macOS 26 or newer" >&2; exit 1; }
        export MACOSX_DEPLOYMENT_TARGET=26.0
        platform=macos-arm64
        ;;
    x86_64-unknown-linux-gnu)
        [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] \
            || { echo "Build Linux x64 artifacts on native Linux x64" >&2; exit 1; }
        platform=linux-x64
        ;;
    aarch64-unknown-linux-gnu)
        [[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] \
            || { echo "Build Linux ARM64 artifacts on native Linux ARM64" >&2; exit 1; }
        platform=linux-arm64
        ;;
    *) echo "Unsupported release target: $target" >&2; exit 1 ;;
esac
cargo build --release --locked --target "$target"
target_dir=$(jq -r .target_directory <<< "$metadata")
asset=tinlynk-editor-$tag-$platform
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
install -m 0755 "$target_dir/$target/release/tinlynk-editor" "$output/$asset"
if [[ "$platform" == macos-arm64 ]]; then
    # Stripping/linking must leave a valid ad-hoc signature on Apple Silicon.
    codesign --force --sign - "$output/$asset"
    mkdir -p "$stage/root/usr/local/bin"
    install -m 0755 "$output/$asset" "$stage/root/usr/local/bin/tinlynk-editor"
    pkgbuild --root "$stage/root" --install-location / \
        --identifier dev.tinlynk.terminal-editor --version "$version" \
        "$stage/component.pkg"
    cat > "$stage/distribution.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>TinLynk Terminal Editor</title>
  <options customize="never" require-scripts="false" hostArchitectures="arm64"/>
  <volume-check>
    <allowed-os-versions><os-version min="26.0"/></allowed-os-versions>
  </volume-check>
  <choices-outline><line choice="default"/></choices-outline>
  <choice id="default" visible="false"><pkg-ref id="dev.tinlynk.terminal-editor"/></choice>
  <pkg-ref id="dev.tinlynk.terminal-editor" version="$version">component.pkg</pkg-ref>
</installer-gui-script>
EOF
    productbuild --distribution "$stage/distribution.xml" \
        --package-path "$stage" "$output/$asset.pkg"
else
    install -m 0755 "$output/$asset" "$stage/tinlynk-editor"
    tar -czf "$output/$asset.tar.gz" -C "$stage" tinlynk-editor
fi
printf 'Packaged %s in %s\n' "$tag" "$output"
