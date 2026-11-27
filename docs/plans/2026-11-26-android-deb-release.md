# Android Deb Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build signed `aarch64` and `arm` Termux `.deb` packages on a `v2.0.0` tag and publish them to a GitHub Release.

**Architecture:** A shell builder stages the cross-compiled binaries into a Termux deb tree and runs `dpkg-deb`. A tag-triggered workflow builds both architectures with `cargo ndk`, signs every deb and `SHA256SUMS` with the existing apt key, and uploads seven assets to the tag release. A manual dispatch builds the same packages without signing or publishing.

**Tech Stack:** Rust 1.100, cargo-ndk, Android NDK r27c, `dpkg-deb`, GPG, GitHub Actions, `gh` CLI.

**Spec:** `docs/specs/2026-11-26-android-deb-release-design.md`

## Global Constraints

- Package version is `2.0.0`; release tag is `v2.0.0`.
- Debian architectures are exactly `aarch64` and `arm`.
- Binaries live at `data/data/com.termux/files/usr/bin/`.
- Control fields are exactly: `Package: termux-sync`, `Depends: dpkg`, `Section: utils`, `Priority: optional`.
- Signature key is fingerprint `476FC230ECBBFDF0`; secrets are `GPG_PRIVATE_KEY` and `GPG_PASSPHRASE`.
- No unsigned release path exists.
- `workflow_dispatch` builds artifacts only; it never signs or publishes.
- Do not change backup, restore, config, event, or TUI behavior.
- Every commit is signed with `git commit -S`; verify with `git verify-commit HEAD`.
- Run `sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"` before commits.
- The tag is created only after the manual-dispatch verification passes.
- No crates.io publish is involved.

## Review Focus

- A deb missing the Termux prefix or with the wrong modes is unusable. Task 1's test extracts the package and runs both binaries.
- The `arm` job must not overwrite or omit the `aarch64` artifact. Task 2 uploads per-architecture artifacts and downloads them with merge.
- Signatures must verify with the public key and cover both debs and `SHA256SUMS`. Task 2 verifies every signature before upload.
- A tag whose version differs from `Cargo.toml` would ship a mislabeled release. Task 2 asserts equality.
- The apt recipe depends on the tag tarball. This plan ends with the tag, then the apt plan starts.

---

### Task 1: Add the deb builder and its packaging test

**Files:**
- Create: `tests/deb.sh`
- Create: `scripts/build-deb.sh`

**Interfaces:**
- Consumes: release binaries in a directory.
- Produces: `scripts/build-deb.sh <debian-arch> <version> <bin-dir> <out-dir>` and `termux-sync_<version>_<arch>.deb`.

- [ ] **Step 1: Write the failing packaging test**

Create `tests/deb.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --locked --release --bins >/dev/null

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

bash scripts/build-deb.sh aarch64 "$VERSION" target/release "$TMP"

DEB="$TMP/termux-sync_${VERSION}_aarch64.deb"
test -f "$DEB"

dpkg-deb -f "$DEB" Package | grep -qx 'termux-sync'
dpkg-deb -f "$DEB" Version | grep -qx "$VERSION"
dpkg-deb -f "$DEB" Architecture | grep -qx 'aarch64'
dpkg-deb -f "$DEB" Depends | grep -qx 'dpkg'

LISTING="$(dpkg-deb -c "$DEB")"
grep -q './data/data/com.termux/files/usr/bin/termux-sync$' <<<"$LISTING"
grep -q './data/data/com.termux/files/usr/bin/termux-sync-tui$' <<<"$LISTING"
grep -q './data/data/com.termux/files/usr/share/doc/termux-sync/README.md$' <<<"$LISTING"

mkdir -p "$TMP/root"
dpkg-deb -x "$DEB" "$TMP/root"
PREFIX="$TMP/root/data/data/com.termux/files/usr/bin"
"$PREFIX/termux-sync" --version | grep -qx "termux-sync v$VERSION"
"$PREFIX/termux-sync-tui" --version | grep -qx "termux-sync-tui $VERSION"

echo "deb packaging ok"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `bash tests/deb.sh`
Expected: failure because `scripts/build-deb.sh` does not exist.

- [ ] **Step 3: Implement `scripts/build-deb.sh`**

```bash
#!/data/data/com.termux/files/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <debian-arch> <version> <bin-dir> <out-dir>" >&2
    exit 2
fi

ARCH="$1"
VERSION="$2"
BIN_DIR="$3"
OUT_DIR="$4"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

case "$ARCH" in
    aarch64|arm) ;;
    *) echo "unsupported arch: $ARCH" >&2; exit 2 ;;
esac

test -x "$BIN_DIR/termux-sync"
test -x "$BIN_DIR/termux-sync-tui"
test -f "$ROOT/README.md"
test -f "$ROOT/LICENSE"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PREFIX_DIR="$STAGE/data/data/com.termux/files/usr"
mkdir -p "$PREFIX_DIR/bin" "$PREFIX_DIR/share/doc/termux-sync" "$STAGE/DEBIAN"

install -m755 "$BIN_DIR/termux-sync" "$PREFIX_DIR/bin/termux-sync"
install -m755 "$BIN_DIR/termux-sync-tui" "$PREFIX_DIR/bin/termux-sync-tui"
install -m644 "$ROOT/README.md" "$PREFIX_DIR/share/doc/termux-sync/README.md"
install -m644 "$ROOT/LICENSE" "$PREFIX_DIR/share/doc/termux-sync/LICENSE"

INSTALLED_SIZE="$(du -sk "$STAGE" | cut -f1)"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: termux-sync
Version: $VERSION
Architecture: $ARCH
Maintainer: Ronald Sloan (for termux) <xkiller1177@gmail.com>
Installed-Size: $INSTALLED_SIZE
Depends: dpkg
Section: utils
Priority: optional
Homepage: https://github.com/GeneralKaos666/termux-sync-rust
Description: Back up and restore a Termux environment across devices
EOF

chmod 755 "$STAGE/DEBIAN"
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT_DIR/termux-sync_${VERSION}_${ARCH}.deb"
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `bash tests/deb.sh`
Expected: `deb packaging ok`.

- [ ] **Step 5: Test the `arm` metadata path with the host binaries**

Run:

```bash
TMP=$(mktemp -d)
bash scripts/build-deb.sh arm 2.0.0 target/release "$TMP"
dpkg-deb -f "$TMP/termux-sync_2.0.0_arm.deb" Architecture
dpkg-deb -c "$TMP/termux-sync_2.0.0_arm.deb" | head
rm -rf "$TMP"
```

Expected: `arm`, and both binaries present in the listing. The binaries are
host-built for this metadata check only.

- [ ] **Step 6: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add scripts/build-deb.sh tests/deb.sh
git commit -S -m "build: add Termux deb builder"
git verify-commit HEAD
```

---

### Task 2: Add the tag release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `scripts/build-deb.sh` from Task 1.
- Produces: GitHub Release assets `termux-sync_2.0.0_{aarch64,arm}.deb`, their `.asc` files, `SHA256SUMS`, `SHA256SUMS.asc`, and `termux-sync-signing-key.asc`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags:
      - "v*"
  workflow_dispatch:
    inputs:
      version:
        description: "Version to build without the leading v"
        required: true
        type: string

permissions:
  contents: write

env:
  GPG_KEY_ID: 476FC230ECBBFDF0

jobs:
  debs:
    name: Deb ${{ matrix.arch }}
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        include:
          - arch: aarch64
            ndk_abi: arm64-v8a
            rust_target: aarch64-linux-android
          - arch: arm
            ndk_abi: armeabi-v7a
            rust_target: armv7-linux-androideabi
    steps:
      - uses: actions/checkout@v7
      - name: Set up Android NDK
        uses: nttld/setup-ndk@v1
        with:
          ndk-version: r27c
      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.rust_target }}
      - name: Install cargo-ndk
        run: cargo install cargo-ndk --locked
      - name: Resolve version
        id: version
        shell: bash
        run: |
          set -euo pipefail
          if [ "$GITHUB_EVENT_NAME" = "push" ]; then
            version="${GITHUB_REF_NAME#v}"
          else
            version="${{ inputs.version }}"
          fi
          test -n "$version"
          echo "version=$version" >> "$GITHUB_OUTPUT"
      - name: Assert version matches Cargo
        shell: bash
        run: |
          set -euo pipefail
          cargo_version="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
          test "$cargo_version" = "${{ steps.version.outputs.version }}"
      - name: Build Android binaries
        run: cargo ndk -t ${{ matrix.ndk_abi }} build --locked --release --bins
      - name: Build deb
        shell: bash
        run: |
          set -euo pipefail
          bash scripts/build-deb.sh \
            "${{ matrix.arch }}" \
            "${{ steps.version.outputs.version }}" \
            "target/${{ matrix.rust_target }}/release" \
            dist
          dpkg-deb -I "dist/termux-sync_${{ steps.version.outputs.version }}_${{ matrix.arch }}.deb"
      - name: Upload deb
        uses: actions/upload-artifact@v4
        with:
          name: deb-${{ matrix.arch }}
          path: dist/*.deb
          if-no-files-found: error

  sign:
    name: Sign and release
    if: github.event_name == 'push'
    needs: debs
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Download debs
        uses: actions/download-artifact@v4
        with:
          pattern: deb-*
          merge-multiple: true
          path: dist
      - name: Import signing key
        env:
          GPG_PRIVATE_KEY: ${{ secrets.GPG_PRIVATE_KEY }}
        run: |
          set -euo pipefail
          export GNUPGHOME="$RUNNER_TEMP/gnupg"
          mkdir -p "$GNUPGHOME"
          chmod 700 "$GNUPGHOME"
          printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import
          echo "GNUPGHOME=$GNUPGHOME" >> "$GITHUB_ENV"
      - name: Sign debs and checksums
        env:
          GPG_PASSPHRASE: ${{ secrets.GPG_PASSPHRASE }}
        run: |
          set -euo pipefail
          for deb in dist/*.deb; do
            printf '%s' "$GPG_PASSPHRASE" | gpg --batch --yes \
              --pinentry-mode loopback --passphrase-fd 0 \
              --armor --detach-sign --local-user "$GPG_KEY_ID" "$deb"
          done
          (cd dist && sha256sum ./*.deb > SHA256SUMS)
          printf '%s' "$GPG_PASSPHRASE" | gpg --batch --yes \
            --pinentry-mode loopback --passphrase-fd 0 \
            --armor --detach-sign --local-user "$GPG_KEY_ID" dist/SHA256SUMS
          gpg --armor --export "$GPG_KEY_ID" > dist/termux-sync-signing-key.asc
      - name: Verify signatures
        run: |
          set -euo pipefail
          gpg --verify dist/SHA256SUMS.asc dist/SHA256SUMS
          (cd dist && sha256sum -c SHA256SUMS)
          for deb in dist/*.deb; do
            gpg --verify "$deb.asc" "$deb"
          done
          test "$(gpg --with-colons --fingerprint "$GPG_KEY_ID" | awk -F: '/^fpr:/ {print $10; exit}')" = "$GPG_KEY_ID"
      - name: Publish release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -euo pipefail
          tag="$GITHUB_REF_NAME"
          gh release view "$tag" >/dev/null 2>&1 || \
            gh release create "$tag" --title "termux-sync $tag" --generate-notes
          gh release upload "$tag" dist/* --clobber
```

- [ ] **Step 2: Check YAML and shell syntax**

Run: `git diff --check`
Expected: exit 0.

Run:

```bash
python3 - <<'PY'
import yaml
with open(".github/workflows/release.yml") as f:
    yaml.safe_load(f)
print("yaml ok")
PY
```

Expected: `yaml ok`. If PyYAML is unavailable, skip this command and rely on
the workflow_dispatch run in Task 4.

- [ ] **Step 3: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add .github/workflows/release.yml
git commit -S -m "ci: build signed Termux debs on tags"
git verify-commit HEAD
```

---

### Task 3: Document installation and release flow

**Files:**
- Modify: `README.md`
- Modify: `FAQ.md`
- Modify: `GOVERNANCE.md`

**Interfaces:**
- Consumes: asset names and tag flow from Task 2.
- Produces: user verification and install instructions.

- [ ] **Step 1: Add the prebuilt package section to `README.md`**

Insert under `## Installation`, after the `cargo install` block:

- Choose the architecture from `uname -m`: `aarch64` or `arm`.
- Download `termux-sync_2.0.0_<arch>.deb`, `termux-sync-signing-key.asc`,
  `SHA256SUMS`, and `SHA256SUMS.asc` from the `v2.0.0` release.
- Verify exactly:

```bash
gpg --import termux-sync-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS
gpg --verify termux-sync_2.0.0_aarch64.deb.asc termux-sync_2.0.0_aarch64.deb
pkg install ./termux-sync_2.0.0_aarch64.deb
```

- Point users at the apt repository for `apt install termux-sync` and link
  `https://github.com/GeneralKaos666/termux-apt-repo`.

- [ ] **Step 2: Add the same verification note to `FAQ.md`**

Add a question `How do I verify a prebuilt .deb?` with the `gpg` and
`sha256sum` commands above.

- [ ] **Step 3: Update `GOVERNANCE.md`**

Add to the release process: after the version bump, push tag `vX.Y.Z`; the
release workflow builds and signs both Termux debs and publishes the GitHub
Release. The apt repository publishes after its PR merges.

- [ ] **Step 4: Check documentation references**

Run:

```bash
git grep -n 'termux-sync_2.0.0_<arch>.deb\|476FC230ECBBFDF0' README.md FAQ.md GOVERNANCE.md
```

Expected: the verification instructions and fingerprint appear.

- [ ] **Step 5: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add README.md FAQ.md GOVERNANCE.md
git commit -S -m "docs: document signed Termux packages"
git verify-commit HEAD
```

---

### Task 4: Merge and run the manual build verification

**Files:** none planned.

**Interfaces:**
- Consumes: Tasks 1–3 on `main`.
- Produces: two verified workflow artifacts before the tag.

- [ ] **Step 1: Run the repository gates**

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash tests/pty.sh
bash tests/deb.sh
```

Expected: every command exits 0.

- [ ] **Step 2: Merge to `main` and push**

Use the finishing-a-development-branch flow. After merge:

```bash
git push origin main
```

- [ ] **Step 3: Start a build-only dispatch**

Run:

```bash
gh workflow run release.yml --ref main -f version=2.0.0
gh run list --workflow release.yml --limit 1
```

Expected: a run starts on `main`. The `sign` job is skipped because the event
is `workflow_dispatch`.

- [ ] **Step 4: Download and test the aarch64 artifact**

Wait for the run to finish, then:

```bash
RUN_ID=$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --exit-status
TMP=$(mktemp -d)
gh run download "$RUN_ID" -n deb-aarch64 -D "$TMP"
dpkg-deb -x "$TMP/termux-sync_2.0.0_aarch64.deb" "$TMP/root"
"$TMP/root/data/data/com.termux/files/usr/bin/termux-sync" --version
"$TMP/root/data/data/com.termux/files/usr/bin/termux-sync-tui" --version
```

Expected:

```text
termux-sync v2.0.0
termux-sync-tui 2.0.0
```

Inspect the `arm` artifact control and contents:

```bash
gh run download "$RUN_ID" -n deb-arm -D "$TMP"
dpkg-deb -f "$TMP/termux-sync_2.0.0_arm.deb" Architecture
dpkg-deb -c "$TMP/termux-sync_2.0.0_arm.deb" | grep termux-sync
```

Expected: `arm`, and both binaries plus docs are listed.

- [ ] **Step 5: Report and stop before tagging**

Report the run URL, versions, and artifact inspection. Do not create the tag
without the next task's explicit step.

---

### Task 5: Tag v2.0.0 and verify the signed release

**Files:** none.

**Interfaces:**
- Consumes: verified manual build.
- Produces: signed GitHub Release `v2.0.0`.

- [ ] **Step 1: Create and push the signed tag**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git tag -s v2.0.0 -m "termux-sync v2.0.0"
git verify-tag v2.0.0
git push origin v2.0.0
```

- [ ] **Step 2: Watch the release workflow**

```bash
gh run list --workflow release.yml --limit 1
RUN_ID=$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --exit-status
```

Expected: `debs` for both architectures and `sign` succeed.

- [ ] **Step 3: Verify the release assets and signatures**

```bash
TMP=$(mktemp -d)
gh release download v2.0.0 -D "$TMP"
ls "$TMP"
gpg --import "$TMP/termux-sync-signing-key.asc"
gpg --verify "$TMP/SHA256SUMS.asc" "$TMP/SHA256SUMS"
(cd "$TMP" && sha256sum -c SHA256SUMS)
gpg --verify "$TMP/termux-sync_2.0.0_aarch64.deb.asc" "$TMP/termux-sync_2.0.0_aarch64.deb"
gpg --verify "$TMP/termux-sync_2.0.0_arm.deb.asc" "$TMP/termux-sync_2.0.0_arm.deb"
gpg --fingerprint 476FC230ECBBFDF0
```

Expected: seven assets; every signature is good; both checksums pass;
fingerprint `476FC230ECBBFDF0`.

- [ ] **Step 4: Report**

Report the release URL and exact verification output. The apt repository work
starts only after this passes.

---

## Follow-Up

The apt repository plan is
`docs/plans/2026-11-26-termux-apt-repo-multiarch.md`. It requires the `v2.0.0`
tag created here.
