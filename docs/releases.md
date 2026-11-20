# Releases and installation

## Platforms and assets

Release tags must match the Cargo package version with a `v` prefix, such as `v0.1.0` or `v0.2.0-rc.1`. Versions with SemVer build metadata (`+...`) are not supported by release tooling.

| Platform | Requirements | Assets for `v0.1.0` |
| --- | --- | --- |
| macOS ARM64 | Apple Silicon, macOS 26+ | `tinlynk-editor-v0.1.0-macos-arm64`, `tinlynk-editor-v0.1.0-macos-arm64.pkg` |
| Linux x64 | GNU/Linux, x86_64, glibc 2.35+ | `tinlynk-editor-v0.1.0-linux-x64`, `tinlynk-editor-v0.1.0-linux-x64.tar.gz` |
| Linux ARM64 | GNU/Linux, aarch64, glibc 2.35+ | `tinlynk-editor-v0.1.0-linux-arm64`, `tinlynk-editor-v0.1.0-linux-arm64.tar.gz` |

Every release also includes `install.sh` and `SHA256SUMS`. Linux archives contain one executable, `tinlynk-editor`, with executable permissions. The macOS pkg installs only `/usr/local/bin/tinlynk-editor`; its distribution metadata restricts installation to ARM64 and macOS 26+. Embedded themes require no separate files. Fonts are optional and are not installed.

Linux builds use native Ubuntu 22.04 runners. These are dynamically linked GNU binaries, not static or musl builds. Alpine Linux, Windows, and Intel Macs are not release targets. A Unicode-capable terminal remains required on every target.

## Shell installer

Requires POSIX `sh`, `curl`, standard Unix file utilities, and either `sha256sum` or `shasum`. The script uses HTTPS GitHub downloads and rejects unsupported OS/architecture combinations and macOS versions below 26. On Linux, ensure `getconf GNU_LIBC_VERSION` reports glibc 2.35 or newer; the installer does not run the downloaded executable to probe compatibility.

Download the installer from the release, review it, then run it:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  -o install.sh https://github.com/tinlynk/terminal-editor/releases/latest/download/install.sh
sh install.sh --help
# After reviewing the script:
sh install.sh
sh install.sh --version v0.1.0 --prefix "$HOME/.local"
sh install.sh --version 0.1.0 --bin-dir "$HOME/tools"
```

- With no version, resolve GitHub's latest published stable release to a concrete tag before downloading. Prereleases require `--version`.
- The default destination is `/usr/local/bin/tinlynk-editor`.
- `--prefix PREFIX` installs into `PREFIX/bin`; `--bin-dir DIRECTORY` uses the exact directory. Do not combine them.
- Relative directories resolve against the working directory. Paths containing spaces are supported.
- The script downloads the raw binary and checksum manifest into a private temporary directory, verifies the exact asset, then installs mode 0755. It cleans up temporary downloads on exit.
- Installation uses normal user permissions when possible. Otherwise only directory creation and copying the verified binary use sudo. Prefer a user-owned prefix if administrator access is unavailable.
- Existing regular executables are replaced. A destination executable that is a directory or symlink is rejected; remove or relocate it explicitly first.
- Download, checksum, platform, argument, and permission errors stop installation and produce diagnostics. Verification failures do not modify the destination.

For a user installation, add the directory to your shell's `PATH`, for example:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

To make this persistent, add it to your shell's startup configuration yourself. The installer does not edit shell profiles.

## Checksums and manual installation

Download assets and `SHA256SUMS` from the **same release**. For example, on Linux x64:

```sh
version=v0.1.0
base="https://github.com/tinlynk/terminal-editor/releases/download/$version"
asset="tinlynk-editor-$version-linux-x64.tar.gz"
curl --fail --location --proto '=https' --proto-redir '=https' -O "$base/$asset"
curl --fail --location --proto '=https' --proto-redir '=https' -O "$base/SHA256SUMS"
awk -v asset="$asset" '$2 == asset' SHA256SUMS > selected-checksum
test "$(wc -l < selected-checksum)" -eq 1
sha256sum --check selected-checksum
tar -xzf "$asset"
mkdir -p "$HOME/.local/bin"
install -m 0755 tinlynk-editor "$HOME/.local/bin/tinlynk-editor"
```

On macOS use `shasum -a 256 --check selected-checksum` instead. Checksum manifests use bare asset filenames, so run verification in the download directory. The examples assume each command succeeds; stop on any error.

For raw binaries, verify the selected asset, then copy it with `install -m 0755 ASSET DESTINATION/tinlynk-editor`. Downloaded raw binaries may not retain executable permissions. For a system-wide manual install, create `/usr/local/bin` if necessary and use sudo for the final copy, not for downloading.

For a verified macOS package, open it in Installer, or run:

```sh
sudo installer -pkg tinlynk-editor-v0.1.0-macos-arm64.pkg -target /
```

The package destination is fixed; use the shell installer or raw binary for custom paths.

**Trust limitations:** SHA-256 checksums detect corruption but are not independent publisher signatures. The installer and checksums come from the same GitHub release. The macOS executable has an ad-hoc code signature for Apple Silicon execution, not a Developer ID signature. The pkg is unsigned, and neither artifact is notarized. Gatekeeper may warn or refuse to open downloaded artifacts. Only proceed if you trust and have verified the download; use macOS's explicit approval mechanism in Privacy & Security where available. Do not disable Gatekeeper globally. Developer ID signing and notarization require a separate future credential/setup change.

## Creating a release

The [release workflow](../.github/workflows/release.yml) builds, tests, and packages all three targets. It publishes only after every platform passes validation and the complete payload is assembled.

1. Update `[package].version` in [Cargo.toml](../Cargo.toml).
2. Run `cargo check` to update the root package version in [Cargo.lock](../Cargo.lock), then inspect the diff; do not intentionally update dependencies for a version bump.
3. Run the validation checks below. Commit the version and intended release changes.
4. Tag that commit with the matching `v` version and push it:

   ```sh
   git tag v0.1.0
   git push origin HEAD
   git push origin v0.1.0
   ```

5. Monitor the Release workflow and inspect the generated GitHub Release assets and notes.

The `v*` tag trigger rejects versions that do not match the manifest. A tag such as `v0.2.0-rc.1` is published as a prerelease and is not selected by the installer's default latest-stable lookup. Use a new version/tag for a new release rather than overwriting published assets.

Build jobs have `contents: read`. Only the publishing job has `contents: write`, using the automatically provided `GITHUB_TOKEN`; no personal token or Apple signing secrets are needed. Repository/organization Actions policies must allow the workflow, referenced actions, ARM64 hosted runners, and token write permission for publication.

Platform assets are uploaded independently, then merged with the installer and one `SHA256SUMS` manifest. The publishing job creates a draft, uploads all assets, and makes the release public only after upload succeeds. Concurrent runs for the same ref are serialized. Existing releases are not overwritten. If publication fails after draft creation, inspect the draft and workflow logs; after confirming it was never published, delete the incomplete draft explicitly before rerunning. Do not delete a published release to work around a failure.

## Manual artifact-only builds

Use Actions > Release > Run workflow, selecting the branch or tag to build, or:

```sh
gh workflow run release.yml --ref main
gh run list --workflow release.yml --limit 5
gh run watch RUN_ID
gh run download RUN_ID --name release-assets --dir release-assets
```

Manual builds use the selected revision's Cargo version. They **never publish**, including when dispatched against a tag. `release-assets` contains the final payload and checksums; `release-macos-arm64`, `release-linux-x64`, and `release-linux-arm64` contain intermediate platform outputs. Actions ZIP downloads do not preserve executable permissions on raw binaries; the tar.gz/pkg and installer set the intended permissions.

No release asset exists for the installer's latest-release lookup until a tagged release has actually been published.

## Local packaging

Run commands from the repository root on the corresponding native OS/architecture. Required tools: stable Rust 1.89+, the locked Cargo dependencies, `jq`, and standard build tools. macOS also requires the Xcode command-line tools (`pkgbuild`, `productbuild`, `codesign`, `otool`); Linux requires a native C toolchain and tar/gzip. Runtime artifact checks require Python 3 and `file`, plus `objdump`/`ldd` on Linux or `pkgutil` on macOS.

```sh
# Apple Silicon Mac with macOS 26+:
rustup target add aarch64-apple-darwin
bash scripts/package-release.sh v0.1.0 aarch64-apple-darwin dist
python3 scripts/check-release.py dist/tinlynk-editor-v0.1.0-macos-arm64

# Native Ubuntu 22.04 x64:
bash scripts/package-release.sh v0.1.0 x86_64-unknown-linux-gnu dist
python3 scripts/check-release.py dist/tinlynk-editor-v0.1.0-linux-x64

# Native Ubuntu 22.04 ARM64:
bash scripts/package-release.sh v0.1.0 aarch64-unknown-linux-gnu dist
python3 scripts/check-release.py dist/tinlynk-editor-v0.1.0-linux-arm64
```

The script uses `cargo build --release --locked --target TARGET`, respects Cargo's target directory, and stages package contents in a temporary directory. It sets `MACOSX_DEPLOYMENT_TARGET=26.0` for macOS builds. Use Ubuntu 22.04 to preserve the Linux glibc baseline; artifacts built on newer distributions may require newer glibc, and the artifact checker rejects that.

To assemble local outputs from all platforms, copy their six assets and `install.sh` into one directory. Generate checksums there with `sha256sum tinlynk-editor-* install.sh > SHA256SUMS` (Linux), or `shasum -a 256 tinlynk-editor-* install.sh > SHA256SUMS` (macOS). Local packaging does not publish anything.

## Validation

```sh
sh -n install.sh
bash -n scripts/package-release.sh scripts/test-install.sh
bash scripts/test-install.sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Installer tests use mocked network/platform/privilege commands, fixture downloads, and temporary installation directories; they do not use the network or install into system paths. They cover platform aliases, latest/pinned/prerelease versions, custom destinations, option errors, checksum failures, download failures, and privilege success/failure. Privilege simulation is skipped if run as a user that can write mode-0555 directories, such as root.

Before publishing, require all native release jobs to pass. The artifact checker inspects native architecture and executable permissions, macOS minimum deployment target/ad-hoc signature/pkg restrictions and payload, Linux glibc symbol requirements/archive contents, and linked libraries. It launches the shipped executable in an isolated PTY and requires successful rendering and exit on Ctrl+Q. It does not install the pkg onto the runner's system.

Local macOS validation alone is not evidence of Linux runtime compatibility. The native Linux release jobs are the required validation gate for Linux assets.

## Uninstall

Remove only the installed executable, for example `rm "$HOME/.local/bin/tinlynk-editor"` or `sudo rm /usr/local/bin/tinlynk-editor`, after confirming its location.

For a pkg installation, `pkgutil --pkg-info dev.tinlynk.terminal-editor` shows the receipt. After removing the executable, `sudo pkgutil --forget dev.tinlynk.terminal-editor` removes that receipt; forgetting a receipt does **not** delete installed files.

Leave `$HOME/.tinlynk/terminal-editor/` and workspace configuration intact unless you explicitly intend to remove your settings.
