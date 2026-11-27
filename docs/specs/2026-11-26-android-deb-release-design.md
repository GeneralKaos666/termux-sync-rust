# Android Deb Release and APT Publishing Design

- **Status:** Approved design, implementation not started
- **Date:** 2026-11-26
- **Scope:** Build signed `aarch64` and `arm` Termux `.deb` packages in CI,
  attach them to a GitHub Release, and publish the same package through
  `GeneralKaos666/termux-apt-repo`.

## Purpose

`termux-sync` currently ships as source and as crates.io crates. There is no
installable Termux package. The Android CI job cross-compiles the binaries but
discards them, and the personal apt repository has no `termux-sync` recipe.

This change builds both Termux architectures on a `v*` tag, signs the
standalone packages, publishes them as GitHub Release assets, and adds the
package to the apt repository for `apt install termux-sync`.

## Goals

1. Build `aarch64` and `arm` `.deb` files from one tag.
2. Publish both packages and their signatures to the GitHub Release.
3. Sign every standalone package and the checksum file with the existing apt
   repository key, fingerprint `476FC230ECBBFDF0`.
4. Add a `termux-sync` recipe to `GeneralKaos666/termux-apt-repo`.
5. Teach the apt repository to build and publish both architectures.
6. Keep the existing apt metadata signing path unchanged.
7. Verify everything before creating the tag.

## Non-Goals

- A single universal package containing both ABIs.
- Changing backup, restore, configuration, event, or TUI behavior.
- Adding a separate signing key.
- Signing embedded `debsigs` metadata.
- Publishing to the official Termux repository.
- Replacing crates.io publishing.

## Current State

- `rust.yml` has an Android compile job for `arm64-v8a` and `armeabi-v7a`, but
  it uploads nothing.
- No `.deb` builder or release workflow exists.
- `Cargo.toml` is version `2.0.0`; no `v2.0.0` tag exists.
- `GPG_PRIVATE_KEY` and `GPG_PASSPHRASE` now exist as Actions secrets in
  `GeneralKaos666/termux-sync-rust`, using the apt repository key.
- `GeneralKaos666/termux-apt-repo` has a working sync workflow, a multi-arch
  `termux-apt-repo` generator, and GPG secrets.
- Its sync workflow builds only `aarch64`.
- Its `sync.py` keeps one deb per package name, ignoring architecture, so two
  architectures would delete each other.
- Its `published_version` reads only `binary-aarch64`, so a missing `arm` build
  would not be detected.
- Its recipe format uses the `termux/termux-packages` build harness.

## Release Artifacts

The `v2.0.0` GitHub Release gets:

```text
termux-sync_2.0.0_aarch64.deb
termux-sync_2.0.0_aarch64.deb.asc
termux-sync_2.0.0_arm.deb
termux-sync_2.0.0_arm.deb.asc
SHA256SUMS
SHA256SUMS.asc
termux-sync-signing-key.asc
```

`SHA256SUMS` lists both `.deb` files only. `termux-sync-signing-key.asc` is the
ASCII-armored public key.

## Standalone Deb Build

Add `scripts/build-deb.sh` with this interface:

```bash
scripts/build-deb.sh <debian-arch> <version> <bin-dir> <out-dir>
```

- `<debian-arch>` is `aarch64` or `arm`.
- `<version>` comes from the tag or manual input and must equal the Cargo
  package version.
- `<bin-dir>` contains the cross-compiled `termux-sync` and
  `termux-sync-tui`.
- Output is `<out-dir>/termux-sync_<version>_<arch>.deb`.

Package staging:

```text
DEBIAN/control
data/data/com.termux/files/usr/bin/termux-sync
data/data/com.termux/files/usr/bin/termux-sync-tui
data/data/com.termux/files/usr/share/doc/termux-sync/README.md
data/data/com.termux/files/usr/share/doc/termux-sync/LICENSE
```

Control fields:

```text
Package: termux-sync
Version: <version>
Architecture: <debian-arch>
Maintainer: Ronald Sloan (for termux) <xkiller1177@gmail.com>
Installed-Size: <KiB from du -sk>
Depends: dpkg
Section: utils
Priority: optional
Homepage: https://github.com/GeneralKaos666/termux-sync-rust
Description: Back up and restore a Termux environment across devices
```

Build with `dpkg-deb --build --root-owner-group`. Binary modes are `0755`.
The script also works locally against `target/release` for packaging tests.

## Release Workflow

Add `.github/workflows/release.yml`.

Triggers:

- `push` tags matching `v*`.
- `workflow_dispatch` with a required `version` input. Manual runs build and
  upload workflow artifacts only; they never sign or publish a release.

`debs` job matrix:

| Debian arch | NDK ABI | Rust target |
|---|---|---|
| `aarch64` | `arm64-v8a` | `aarch64-linux-android` |
| `arm` | `armeabi-v7a` | `armv7-linux-androideabi` |

Steps:

1. Checkout.
2. Set up NDK `r27c`.
3. Set up Rust with the matrix target.
4. Install `cargo-ndk --locked`.
5. Resolve `version`: tag name with leading `v` removed, or the manual input.
6. Assert `version` equals `cargo metadata --no-deps` version.
7. Run `cargo ndk -t <abi> build --locked --release --bins`.
8. Run `scripts/build-deb.sh <arch> <version> target/<rust-target>/release dist`.
9. Upload the `.deb` as a workflow artifact.

Tag runs add a `sign` job:

1. Download both deb artifacts into `dist/`.
2. Import `GPG_PRIVATE_KEY` into a temporary `GNUPGHOME`.
3. For each `.deb`, create an ASCII-armored detached signature with
   `gpg --batch --yes --pinentry-mode loopback --passphrase-fd 0`.
4. Write `SHA256SUMS` over the two debs.
5. Sign `SHA256SUMS` as `SHA256SUMS.asc`.
6. Export the public key as `termux-sync-signing-key.asc`.
7. Verify each signature with `gpg --verify` before upload.
8. Create the release if absent, otherwise reuse it, and upload all assets with
   `--clobber`.

Missing or invalid secrets fail the job. There is no unsigned release path.

## Apt Repository Recipe

Add `packages/termux-sync/build.sh` to `GeneralKaos666/termux-apt-repo`:

```text
TERMUX_PKG_HOMEPAGE=https://github.com/GeneralKaos666/termux-sync-rust
TERMUX_PKG_DESCRIPTION="Back up and restore a Termux environment across devices"
TERMUX_PKG_LICENSE="MIT"
TERMUX_PKG_MAINTAINER="Ronald Sloan (for termux) <xkiller1177@gmail.com>"
TERMUX_PKG_VERSION="2.0.0"
TERMUX_PKG_SRCURL=https://github.com/GeneralKaos666/termux-sync-rust/archive/refs/tags/v2.0.0.tar.gz
TERMUX_PKG_SHA256=<sha256 of the tag tarball>
TERMUX_PKG_DEPENDS="dpkg"
TERMUX_PKG_BUILD_IN_SRC=true
```

Build:

```bash
termux_step_make() {
    termux_setup_rust
    cargo build --release --locked --target "$CARGO_TARGET_NAME"
}

termux_step_make_install() {
    install -Dm755 "target/$CARGO_TARGET_NAME/release/termux-sync" \
        "$TERMUX_PREFIX/bin/termux-sync"
    install -Dm755 "target/$CARGO_TARGET_NAME/release/termux-sync-tui" \
        "$TERMUX_PREFIX/bin/termux-sync-tui"
    install -Dm644 README.md "$TERMUX_PREFIX/share/doc/termux-sync/README.md"
    install -Dm644 LICENSE "$TERMUX_PREFIX/share/doc/termux-sync/LICENSE"
}
```

The checksum is filled from the real tag tarball before the apt PR is merged.
Future updates are computed by `sync.py` through `_sha256_of_url`.

## Apt Multi-Arch Support

`sync.json` adds:

```json
{
  "name": "termux-sync",
  "upstream": "GeneralKaos666/termux-sync-rust",
  "detect": "latest_tag",
  "architectures": ["aarch64", "arm"]
}
```

`scripts/sync.py` changes:

1. `latest_tag` strips `v` for `termux-sync`, like `termux-settings`.
2. Add `architectures(name)` returning the configured list, default
   `["aarch64"]`.
3. Add `published_version_for(name, arch, root)` reading
   `binary-<arch>/Packages`; return `None` when the package is absent.
4. `needs_update` returns true when any configured architecture is absent or
   older than the upstream version.
5. `_dedupe_latest` groups by `(package, architecture)` and keeps the newest
   version for each pair.
6. `bump` handles `termux-sync`: update version, source URL
   `.../refs/tags/<tag>.tar.gz`, and SHA256.
7. Add an `arches <name>` subcommand for the workflow.

`scripts/test_sync.py` gains tests:

- both architectures survive staging;
- a missing `arm` entry makes `needs_update` true;
- `termux-sync` tag detection strips `v`;
- `termux-sync` bump updates version, URL, and checksum;
- existing single-arch tests still pass.

`sync.yml` changes:

1. Run `python3 -m unittest scripts/test_sync.py` before detection.
2. For each changed package, read its architectures and run
   `TARGET_ARCH=<arch> scripts/build-package.sh <pkg>` once per architecture.
3. Keep the existing staging, signing, and push-back steps.

README changes:

- Add `termux-sync` to the package table with both architectures.
- Add `apt install termux-sync` to the quick start.

## User-Facing Documentation

`README.md` and `FAQ.md` gain a prebuilt-package section:

- How to choose `aarch64` or `arm` from `uname -m`.
- Download the deb, key, and checksums from the GitHub Release.
- Verify:

```bash
gpg --import termux-sync-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS
gpg --verify termux-sync_2.0.0_aarch64.deb.asc termux-sync_2.0.0_aarch64.deb
pkg install ./termux-sync_2.0.0_aarch64.deb
```

- Apt repository commands: import the existing apt key, add the source list,
  `apt update`, `apt install termux-sync`.

`GOVERNANCE.md` release steps mention the tag-driven release workflow.

## Release Sequence

1. Implement this repository's builder and workflow. Run local packaging tests.
2. Push a branch and run `workflow_dispatch` in build-only mode. Download the
   `aarch64` artifact, extract it, and run both `--version` commands on this
   device.
3. Implement `termux-apt-repo` changes and run its Python tests. Open a PR.
4. Merge this repository's changes to `main`.
5. Push tag `v2.0.0`. The release workflow builds both debs, signs them, and
   creates the GitHub Release.
6. Download the `arm` and `aarch64` debs and verify signatures and checksums.
7. Compute the tag tarball SHA256, update the apt PR recipe, and merge it.
8. Let the apt sync workflow build both architectures and publish.
9. Verify `Packages` exists for `aarch64` and `arm`, and that the deb pool
   contains both packages.

## Verification

Local, before any tag:

- `bash -n scripts/build-deb.sh`
- Build host release binaries, package as `aarch64`, run
  `dpkg-deb -I` and `dpkg-deb -x`, then run both extracted binaries with
  `--version`.
- Inspect the `arm` package layout and control by packaging a copied binary
  set; it cannot run on the host.
- `cargo test --locked`, clippy, and fmt remain green.
- Apt repo: `python3 -m unittest scripts/test_sync.py`.
- Apt repo: `python3 scripts/sync.py detect termux-sync` after the tag exists.

CI, before the tag:

- `workflow_dispatch` build for both architectures succeeds.
- Downloaded `aarch64` deb installs or extracts and both binaries run.

After the tag:

- Release has seven assets.
- `gpg --verify SHA256SUMS.asc SHA256SUMS` succeeds.
- `sha256sum -c SHA256SUMS` succeeds.
- Each `.deb.asc` verifies.
- Public key fingerprint is `476FC230ECBBFDF0`.

After the apt merge:

- `dists/stable/main/binary-aarch64/Packages` lists `termux-sync`.
- `dists/stable/main/binary-arm/Packages` lists `termux-sync`.
- Both pool debs exist and match the release version.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| NDK cross-build differs from Termux binaries | Manual dispatch, extract and run the `aarch64` deb on this device before tagging |
| Tag and Cargo versions diverge | Release job asserts tag version equals Cargo metadata |
| Two architectures delete each other in staging | Dedupe by package and architecture, covered by a unit test |
| Missing `arm` build goes unnoticed | `needs_update` checks every configured architecture |
| Signature uses the wrong key | Pin fingerprint `476FC230ECBBFDF0` and verify in CI |
| Secrets missing | Signing job fails; no unsigned release |
| Apt recipe source URL dead | Tag first, then compute the tarball checksum and merge the apt PR |
| Apt workflow breaks other packages | Default architecture remains `aarch64`; only `termux-sync` declares two |
| Release rerun duplicates assets | Reuse the release and upload with `--clobber` |

## Acceptance Criteria

- GitHub Release `v2.0.0` contains both signed debs, both detached signatures,
  `SHA256SUMS`, `SHA256SUMS.asc`, and the public key.
- Signatures verify against fingerprint `476FC230ECBBFDF0`.
- Both debs have the correct Termux data path and control fields.
- The `aarch64` deb runs on this device.
- `termux-apt-repo` unit tests pass.
- Apt `Packages` files list `termux-sync` for `aarch64` and `arm`.
- `apt install termux-sync` works from the repo.
- No backup, restore, config, event, or TUI behavior changes.

## Rollback

- Delete the GitHub Release assets or the tag release if the package is broken.
- Revert the apt PR and let the sync workflow rebuild metadata from the pool.
- The apt repository keeps its previous signed metadata in git history.
