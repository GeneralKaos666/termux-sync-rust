# Termux APT Multi-Arch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `termux-sync` to `GeneralKaos666/termux-apt-repo` and publish it for both `aarch64` and `arm`.

**Architecture:** Add a Termux source recipe that builds both binaries. Teach `sync.py` about per-package architectures, missing-architecture detection, and per-architecture deduplication. Update the sync workflow to build each declared architecture and run its unit tests. Deliver through a branch and PR.

**Tech Stack:** Python 3, unit tests, `termux/termux-packages` harness, GitHub Actions, `termux-apt-repo` generator, GPG.

**Spec:** `docs/specs/2026-11-26-android-deb-release-design.md` in `GeneralKaos666/termux-sync-rust`.

## Global Constraints

- Package name is `termux-sync`; version is `2.0.0`; upstream tag is `v2.0.0`.
- Debian architectures are exactly `aarch64` and `arm`.
- Existing packages keep their current behavior and default to `aarch64`.
- The apt repository keeps signing metadata with fingerprint `476FC230ECBBFDF0`.
- The existing `sync.yml` GPG secrets and push-back behavior stay unchanged.
- Every commit is signed with `git commit -S`; verify with `git verify-commit HEAD`.
- Run `sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"` before commits.
- Work from clean `main`; the pre-existing stash `pre-termux-sync-multiarch` is not part of this work.
- Do not merge the PR until the `v2.0.0` release exists.
- No changes to the `termux-sync` application code.

## Review Focus

- Two architectures must survive staging. A unit test keeps one deb per package and arch.
- A missing `arm` package must be detected even when `aarch64` is current. A unit test covers the missing directory.
- The `bin/` files must come from the tag, not the working tree. The recipe uses the tag tarball.
- The workflow must not build `arm` for existing `aarch64`-only packages. The default remains one architecture.
- The recipe must install both binaries and depend on `dpkg`.
- The PR must not include the stashed sync/mirror work.

---

### Task 1: Prepare the apt repository branch

**Files:** none.

**Interfaces:**
- Consumes: the `v2.0.0` GitHub Release from the release plan.
- Produces: clean branch `feat/termux-sync-multiarch`.

- [ ] **Step 1: Confirm the release exists**

```bash
gh release view v2.0.0 --repo GeneralKaos666/termux-sync-rust
```

Expected: the release exists with both signed debs.

- [ ] **Step 2: Confirm the repository state**

```bash
cd ~/repos/termux-apt-repo
git status --short
git stash list
git fetch origin
git switch main
git pull --ff-only
```

Expected: no working-tree changes; `stash@{0}` is
`pre-termux-sync-multiarch`; `main` is current.

- [ ] **Step 3: Create the feature branch**

```bash
git switch -c feat/termux-sync-multiarch
```

- [ ] **Step 4: Capture the tag tarball checksum**

```bash
curl -fsSL \
  https://github.com/GeneralKaos666/termux-sync-rust/archive/refs/tags/v2.0.0.tar.gz \
  | sha256sum
```

Record the 64-character hex value for Task 2.

---

### Task 2: Add the termux-sync recipe

**Files:**
- Create: `packages/termux-sync/build.sh`

**Interfaces:**
- Consumes: the tag checksum from Task 1.
- Produces: a recipe the Termux harness can build for `aarch64` and `arm`.

- [ ] **Step 1: Write the recipe**

Create `packages/termux-sync/build.sh`:

```bash
TERMUX_PKG_HOMEPAGE=https://github.com/GeneralKaos666/termux-sync-rust
TERMUX_PKG_DESCRIPTION="Back up and restore a Termux environment across devices"
TERMUX_PKG_LICENSE="MIT"
TERMUX_PKG_MAINTAINER="Ronald Sloan (for termux) <xkiller1177@gmail.com>"
TERMUX_PKG_VERSION="2.0.0"
TERMUX_PKG_SRCURL="https://github.com/GeneralKaos666/termux-sync-rust/archive/refs/tags/v2.0.0.tar.gz"
TERMUX_PKG_SHA256="<checksum from Task 1>"
TERMUX_PKG_DEPENDS="dpkg"
TERMUX_PKG_BUILD_IN_SRC=true

termux_step_make() {
	termux_setup_rust
	cargo build --release --locked --target "${CARGO_TARGET_NAME}"
}

termux_step_make_install() {
	install -Dm755 "target/${CARGO_TARGET_NAME}/release/termux-sync" \
		"${TERMUX_PREFIX}/bin/termux-sync"
	install -Dm755 "target/${CARGO_TARGET_NAME}/release/termux-sync-tui" \
		"${TERMUX_PREFIX}/bin/termux-sync-tui"
	install -Dm644 README.md \
		"${TERMUX_PREFIX}/share/doc/termux-sync/README.md"
	install -Dm644 LICENSE \
		"${TERMUX_PREFIX}/share/doc/termux-sync/LICENSE"
}
```

- [ ] **Step 2: Check the recipe syntax**

Run:

```bash
bash -n packages/termux-sync/build.sh
grep -n 'TERMUX_PKG_VERSION\|TERMUX_PKG_SRCURL\|TERMUX_PKG_DEPENDS' \
  packages/termux-sync/build.sh
```

Expected: syntax OK; version `2.0.0`, tag URL, and `dpkg` dependency present.

- [ ] **Step 3: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add packages/termux-sync/build.sh
git commit -S -m "feat(packages): add termux-sync recipe"
git verify-commit HEAD
```

---

### Task 3: Add multi-arch detection and dedupe

**Files:**
- Modify: `scripts/test_sync.py`
- Modify: `scripts/sync.py`

**Interfaces:**
- Consumes: `sync.json` entries from Task 4.
- Produces: `architectures(name, root=".")`, `published_version_for(name, arch, root=".")`, the `arches` subcommand, and per-`(package, arch)` dedupe.

- [ ] **Step 1: Write failing tests**

Append to `scripts/test_sync.py`:

```python
class TestMultiArch(unittest.TestCase):
    def _root(self, tmp, arches=("aarch64", "arm"), version="2.0.0-0"):
        with open(os.path.join(tmp, "sync.json"), "w") as f:
            json.dump({"packages": [{
                "name": "termux-sync",
                "upstream": "GeneralKaos666/termux-sync-rust",
                "detect": "latest_tag",
                "architectures": list(arches),
            }]}, f)
        pkg = os.path.join(tmp, "packages", "termux-sync")
        os.makedirs(pkg)
        with open(os.path.join(pkg, "build.sh"), "w") as f:
            f.write('TERMUX_PKG_VERSION="2.0.0"\n')
        for arch in arches:
            dist = os.path.join(tmp, "docs", "dists", "stable", "main",
                                "binary-" + arch)
            os.makedirs(dist, exist_ok=True)
            with open(os.path.join(dist, "Packages"), "w") as f:
                f.write("Package: termux-sync\nVersion: %s\n\n" % version)

    def test_architectures_from_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._root(tmp)
            self.assertEqual(
                sync.architectures("termux-sync", root=tmp),
                ["aarch64", "arm"])

    def test_missing_arm_needs_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._root(tmp, arches=("aarch64",))
            self.assertTrue(sync.needs_update(
                "termux-sync", {"new": "2.0.0"}, root=tmp))

    def test_both_current_no_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._root(tmp)
            self.assertFalse(sync.needs_update(
                "termux-sync", {"new": "2.0.0"}, root=tmp))

    def test_older_arm_needs_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._root(tmp)
            arm = os.path.join(tmp, "docs", "dists", "stable", "main",
                               "binary-arm", "Packages")
            with open(arm, "w") as f:
                f.write("Package: termux-sync\nVersion: 1.9.0-0\n\n")
            self.assertTrue(sync.needs_update(
                "termux-sync", {"new": "2.0.0"}, root=tmp))

    def test_dedupe_keeps_each_architecture(self):
        with tempfile.TemporaryDirectory() as tmp:
            debs = os.path.join(tmp, "debs")
            os.makedirs(debs)
            for name in (
                "termux-sync_2.0.0_aarch64.deb",
                "termux-sync_2.0.0_arm.deb",
                "termux-sync_1.9.0_aarch64.deb",
                "termux-sync_1.9.0_arm.deb",
            ):
                with open(os.path.join(debs, name), "w") as f:
                    f.write("x")
            sync._dedupe_latest(debs)
            self.assertEqual(sorted(os.listdir(debs)), [
                "termux-sync_2.0.0_aarch64.deb",
                "termux-sync_2.0.0_arm.deb",
            ])
```

Also add to `fake_fetch`:

```python
if "termux-sync-rust/tags" in url:
    return [{"name": "v2.0.0"}]
```

Add this method inside `TestFetchLatest`:

```python
def test_termux_sync_new_strips_v(self):
    info = sync.fetch_latest(
        {"name": "termux-sync", "detect": "latest_tag",
         "upstream": "GeneralKaos666/termux-sync-rust"},
        fetch=fake_fetch)
    self.assertEqual(info["new"], "2.0.0")
    self.assertEqual(info["tag"], "v2.0.0")
```

Also add to `TestBump`:

```python
    def test_termux_sync_bump(self):
        with tempfile.TemporaryDirectory() as tmp:
            _copy_pkg(tmp, "termux-sync")
            sync.bump("termux-sync", {
                "new": "2.0.0",
                "tag": "v2.0.0",
                "sha256": "NEWSHA256",
            }, root=tmp)
            build = _read(os.path.join(tmp, "packages", "termux-sync", "build.sh"))
            self.assertIn('TERMUX_PKG_VERSION="2.0.0"', build)
            self.assertIn(
                'TERMUX_PKG_SRCURL="https://github.com/GeneralKaos666/termux-sync-rust/archive/refs/tags/v2.0.0.tar.gz"',
                build)
            self.assertIn('TERMUX_PKG_SHA256="NEWSHA256"', build)
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `python3 scripts/test_sync.py -v`
Expected: failures for missing `architectures`, missing `sync.py` behavior, and
the dedupe assertion.

- [ ] **Step 3: Implement `latest_tag` and the recipe branch**

In `fetch_latest`, the `latest_tag` branch returns the tag with `v` stripped
for `termux-sync` as well as `termux-settings`.

In `bump`, add:

```python
elif name == "termux-sync":
    build = os.path.join(pkg_dir, "build.sh")
    src_url = ("https://github.com/GeneralKaos666/termux-sync-rust/"
               "archive/refs/tags/%s.tar.gz" % latest["tag"])
    sha = latest.get("sha256") or _sha256_of_url(src_url, download)
    _set_in_file(build, "TERMUX_PKG_VERSION", latest["new"])
    _set_in_file(build, "TERMUX_PKG_SRCURL", src_url)
    _set_in_file(build, "TERMUX_PKG_SHA256", sha)
```

- [ ] **Step 4: Implement architecture-aware version reads**

Change `_config_for` to read the config from the requested root:

```python
def _config_for(name, root="."):
    cfg = load_config(os.path.join(root, "sync.json"))
    for pkg in cfg["packages"]:
        if pkg["name"] == name:
            return pkg
    raise ValueError("unknown package %r" % name)
```

Add:

```python
def architectures(name, root="."):
    return _config_for(name, root).get("architectures", ["aarch64"])


def published_version_for(name, arch, root="."):
    path = os.path.join(root, "docs", "dists", "stable", "main",
                        "binary-" + arch, "Packages")
    if not os.path.exists(path):
        return None
    with open(path) as f:
        content = f.read()
    for block in content.split("\n\n"):
        pkg = re.search(r"^Package:\s*(.+)$", block, re.M)
        if pkg and pkg.group(1).strip() == name:
            ver = re.search(r"^Version:\s*(.+)$", block, re.M)
            if ver:
                return re.sub(r"-[^-]*$", "", ver.group(1).strip())
    return None
```

Keep `published_version(name, root=".")` as a display helper that returns
`published_version_for(name, "aarch64", root)` or `recipe_version`.

Replace `needs_update` with:

```python
def needs_update(name, latest, root="."):
    if latest is None:
        return False
    for arch in architectures(name, root):
        current = published_version_for(name, arch, root)
        if current is None or _version_lt(current, latest["new"]):
            return True
    return False
```

Update existing tests that patch `published_version` to patch
`published_version_for` instead, returning `None` where a package is absent.

- [ ] **Step 5: Implement per-architecture dedupe**

Replace the group key in `_dedupe_latest` with `(name, arch)`:

```python
    by_key = {}
    for deb in glob.glob(os.path.join(debs_dir, "*.deb")):
        base = os.path.basename(deb)
        m = re.match(r"(.+?)_([^_]+)_([^_]+)\.deb$", base)
        if not m:
            continue
        name, version, arch = m.groups()
        by_key.setdefault((name, arch), []).append((version, deb))
    for entries in by_key.values():
        entries.sort(key=functools.cmp_to_key(
            lambda a, b: _dpkg_version_cmp(a[0], b[0])))
        for _version, deb in entries[:-1]:
            os.unlink(deb)
```

- [ ] **Step 6: Add the `arches` subcommand**

```python
    ar = sub.add_parser("arches")
    ar.add_argument("name")
```

and:

```python
    if args.cmd == "arches":
        print("\n".join(architectures(args.name)))
        return 0
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `python3 scripts/test_sync.py -v`
Expected: all tests pass.

- [ ] **Step 8: Add the config entry**

Update `sync.json`:

```json
{ "name": "termux-sync", "upstream": "GeneralKaos666/termux-sync-rust", "detect": "latest_tag", "architectures": ["aarch64", "arm"] }
```

Run:

```bash
python3 scripts/sync.py arches termux-sync
```

Expected:

```text
aarch64
arm
```

- [ ] **Step 9: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add scripts/sync.py scripts/test_sync.py sync.json
git commit -S -m "feat(sync): support per-package architectures"
git verify-commit HEAD
```

---

### Task 4: Build every declared architecture in CI

**Files:**
- Modify: `.github/workflows/sync.yml`

**Interfaces:**
- Consumes: the `arches` subcommand from Task 3.
- Produces: a sync run that builds `termux-sync` for `aarch64` and `arm`.

- [ ] **Step 1: Run the unit tests in CI**

Add after `actions/setup-python`:

```yaml
      - name: Test sync scripts
        run: python3 scripts/test_sync.py
```

Also add `scripts/test_sync.py` to the `push.paths` list so edits to the tests
trigger the workflow.

- [ ] **Step 2: Loop architectures in the build step**

Replace the single build call inside the `while` loop with:

```bash
for arch in $(python3 scripts/sync.py arches "$pkg"); do
  echo "===== build $pkg ($arch) ====="
  TARGET_ARCH="$arch" scripts/build-package.sh "$pkg"
done
```

Keep the existing `TARGET_ARCH` environment default or remove it; the loop
always sets the value for each build. Keep the bump, staging, signing, and
push-back steps unchanged.

- [ ] **Step 3: Check the workflow and script**

```bash
bash -n scripts/build-package.sh
python3 -m py_compile scripts/sync.py scripts/test_sync.py
git diff --check
```

Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add .github/workflows/sync.yml
git commit -S -m "ci(sync): build each package architecture"
git verify-commit HEAD
```

---

### Task 5: Document the package

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: package name and both architectures.
- Produces: apt install instructions.

- [ ] **Step 1: Add the package row**

Add to the package table:

```text
| `termux-sync` | Back up and restore a Termux environment across devices | `aarch64`, `arm` |
```

- [ ] **Step 2: Add install instructions**

Add `termux-sync` to the quick-start install line and add:

```bash
apt install termux-sync
```

The existing key import and source-list steps stay unchanged.

- [ ] **Step 3: Commit**

```bash
sh "$HOME/.config/opencode/skills/gpg-commit/scripts/unlock-gpg.sh"
git add README.md
git commit -S -m "docs: document the termux-sync package"
git verify-commit HEAD
```

---

### Task 6: Verify, open the PR, and publish

**Files:** none planned.

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: merged apt changes and published multi-arch metadata.

- [ ] **Step 1: Run the full unit suite**

Run: `python3 scripts/test_sync.py -v`
Expected: all tests pass.

- [ ] **Step 2: Verify detection against the real tag**

Run: `python3 scripts/sync.py detect termux-sync`

Expected: `new` is `2.0.0`, `changed` is true because no apt package exists.

- [ ] **Step 3: Push the branch and open the PR**

```bash
git push -u origin feat/termux-sync-multiarch
gh pr create --repo GeneralKaos666/termux-apt-repo \
  --base main --head feat/termux-sync-multiarch \
  --title "feat: publish termux-sync for aarch64 and arm" \
  --body "Adds the termux-sync recipe and multi-arch sync support. Verified with python3 scripts/test_sync.py. The v2.0.0 release is already published."
```

Expected: PR URL printed.

- [ ] **Step 4: Review and merge**

Inspect the PR diff for only:

- `packages/termux-sync/build.sh`
- `scripts/sync.py`
- `scripts/test_sync.py`
- `sync.json`
- `.github/workflows/sync.yml`
- `README.md`

Merge only after the diff is limited to those files:

```bash
gh pr merge --repo GeneralKaos666/termux-apt-repo \
  --squash --delete-branch feat/termux-sync-multiarch
```

- [ ] **Step 5: Watch the sync workflow**

```bash
gh run list --repo GeneralKaos666/termux-apt-repo --workflow sync.yml --limit 1
RUN_ID=$(gh run list --repo GeneralKaos666/termux-apt-repo --workflow sync.yml --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --repo GeneralKaos666/termux-apt-repo --exit-status
```

Expected: the run builds both architectures, signs, and pushes.

- [ ] **Step 6: Verify both apt architectures**

```bash
curl -fsSL https://generalkaos666.github.io/termux-apt-repo/dists/stable/main/binary-aarch64/Packages | grep -A8 '^Package: termux-sync$'
curl -fsSL https://generalkaos666.github.io/termux-apt-repo/dists/stable/main/binary-arm/Packages | grep -A8 '^Package: termux-sync$'
```

Expected: both list version `2.0.0-0` with `Filename: pool/main/t/termux-sync/...`.

- [ ] **Step 7: Verify the pool files**

```bash
curl -fsSI https://generalkaos666.github.io/termux-apt-repo/pool/main/t/termux-sync/termux-sync_2.0.0-0_aarch64.deb
curl -fsSI https://generalkaos666.github.io/termux-apt-repo/pool/main/t/termux-sync/termux-sync_2.0.0-0_arm.deb
```

Expected: HTTP 200 for both.

- [ ] **Step 8: Restore the stashed work**

Only after the publish is verified:

```bash
git switch main
git pull --ff-only
git stash list
```

Report the stash name and leave restoration to the user unless they ask. Do
not apply it if it conflicts with the merged multi-arch changes.

- [ ] **Step 9: Report**

Report the PR URL, workflow run, both `Packages` snippets, and the two pool
URLs. No tag or release changes happen in this plan.
