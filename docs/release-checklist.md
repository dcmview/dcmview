# Release Checklist

Use this checklist for every tagged `dcmview` release. The release candidate is
not ready to tag until the exact commit has passed CI. Never move or overwrite a
published tag; if a code defect is found after tagging, prepare a patch release.

Channel configuration, the release workflow's jobs, and artifact descriptions
are in [Reference: Channels And Automation](#reference-channels-and-automation)
at the end of this checklist.

## 1. Establish The Release Baseline

- [ ] Fetch current branches and tags without changing the working tree:

  ```bash
  git fetch origin --prune --tags
  ```

- [ ] Identify the latest stable release tag. Treat its commit—not the previous
      ordinary commit—as the release-note baseline:

  ```bash
  LAST_RELEASE="$(git describe --tags --abbrev=0)"
  git show --no-patch --format=fuller "$LAST_RELEASE"
  ```

- [ ] Review every change between that release and the proposed release
      candidate:

  ```bash
  git log --first-parent --stat "$LAST_RELEASE"..HEAD
  git diff --stat "$LAST_RELEASE"..HEAD
  git diff --name-status "$LAST_RELEASE"..HEAD
  ```

- [ ] Inspect merged pull requests, user-visible behavior, API or CLI changes,
      platform support, dependency/security changes, documentation changes,
      deprecations, and known limitations. Do not derive notes from commit
      subjects alone.
- [ ] Confirm the intended version and release scope. Defer unrelated work
      rather than expanding the release candidate late in the process.

## 2. Prepare Every Release-Note Surface

- [ ] Move the relevant entries from `CHANGELOG.md` **Unreleased** into a dated
      version section. Cover CLI, viewer, Python, API, packaging, and
      documentation changes as applicable.
- [ ] Update `vscode/CHANGELOG.md` with Marketplace-facing extension changes.
      Product-wide extension changes should appear in both changelogs.
- [ ] Prepare the GitHub Release notes from the same factual inventory. Include
      highlights, fixes, compatibility or migration information, known
      limitations, install links, and acknowledgements where appropriate.
- [ ] Prepare the corresponding stable-release notes for the `dcmview-docs`
      repository. Do not present the release as current there until GitHub marks
      it as a non-draft, non-prerelease release.
- [ ] Update `README.md`, `vscode/README.md`, API documentation, compatibility
      statements, and other public pages when the release changes their claims.
- [ ] Cross-check all surfaces for the same version, feature names, support
      boundaries, dates, links, and safety language.

## 3. Run The Publication Update Checklist

### Version And Package Metadata

- [ ] Set the release version consistently in `Cargo.toml`, every
      `crates/*/Cargo.toml`, `pyproject.toml`, `frontend/package.json`, and
      `vscode/package.json`.
- [ ] Regenerate `Cargo.lock`, `frontend/package-lock.json`, and
      `vscode/package-lock.json`; do not hand-edit resolved dependency records.
      The version check covers the root package in Cargo.lock, each
      workspace member manifest, and both the top-level and root-package
      versions in each npm lockfile. A stale member entry in Cargo.lock is
      caught by the `--locked` builds, not by the version check.
- [ ] Check canonical package-version parity and the proposed tag:

  ```bash
  VERSION="$(python scripts/check_versions.py --print-version)"
  python scripts/check_versions.py --tag "v${VERSION}"
  ```

- [ ] Review package descriptions, supported-platform lists, installation
      commands, publisher identifiers, and release-channel configuration.

### Screenshots, GIFs, And Attribution

- [ ] Install the pinned tools and validate the tracked capture configuration as
      described in the [marketing media workflow](marketing-media.md):

  ```bash
  python -m pip install -r marketing/requirements.txt
  npm --prefix marketing ci
  npm --prefix marketing run install-browser
  python scripts/marketing_media.py validate
  ```

- [ ] Retrieve missing public sources, then verify every source file against its
      recorded collection, DOI, license, Series identity, size, and checksum.
      Keep source payloads and both linkage records untracked:

  ```bash
  python scripts/marketing_media.py fetch
  python scripts/marketing_media.py verify-sources
  ```

- [ ] On the clean release-candidate commit, recreate the complete review
      bundle from the real binary and pinned VS Code host:

  ```bash
  python scripts/marketing_media.py capture
  ```

- [ ] Review every image and animation for identifiers, local paths, hostnames,
      misleading clinical implications, rendering errors, obsolete UI, smooth
      playback, and appropriate per-frame dwell time. Confirm the MR/SEG GIF
      visibly overlays the declared source images and the VS Code GIF shows the
      real Explorer **Open with dcmview** action.
- [ ] Verify the approved bundle. This rejects dirty captures, input drift,
      changed source records, modified outputs, and incomplete attribution:

  ```bash
  python scripts/marketing_media.py verify
  ```

- [ ] Publish the complete approved set to every consuming surface from one
      bundle, then review the changes in both repositories:

  ```bash
  VERSION="$(python scripts/check_versions.py --print-version)"
  python scripts/marketing_media.py publish --tag "v${VERSION}" \
    --docs-repo ../dcmview-docs --approve
  python scripts/check.py marketing
  ```

- [ ] Ensure Marketplace README image URLs resolve through HTTPS and are pinned
      to the release tag rather than `main`.
- [ ] Confirm the generated media lock records the dcmview version and commit,
      source/capture/inventory hashes, capture date, output hashes and
      dimensions, tool versions, and modification summaries. Human visual
      approval remains required even when every automated check passes.

### Documentation And Links

- [ ] Check that public examples use the release version and supported commands.
- [ ] Check GitHub, PyPI, VS Code Marketplace, Open VSX, documentation, DOI,
      license, and download links.
- [ ] Confirm new public files are included in source archives, wheels, and VSIX
      packages where intended, and excluded where they are not needed.
- [ ] Confirm no DICOM source payload, credential, local-path linkage file, PHI,
      or sensitive log has become tracked or packaged.

## 4. Build And Verify The Final Release Candidate

- [ ] Regenerate committed fixtures if their generator or expected output
      changed:

  ```bash
  cargo run --locked --example generate_test_fixtures
  ```

- [ ] Run the CI-aligned core profile:

  ```bash
  python scripts/check.py core --install
  ```

- [ ] Run the real-process and VS Code Electron profile on a capable host:

  ```bash
  python scripts/check.py e2e --install
  ```

- [ ] Run the independent network-backed fixture profile when the release
      changes discovery, metadata, codecs, pixels, or upstream compatibility:

  ```bash
  python scripts/check.py external --install
  ```

- [ ] Perform any release-specific manual browser, platform, semantic, WSI,
      annotation, or Marketplace checks that automated profiles do not cover.
      Remote use needs no manual check: the `remote-ssh` and
      `vscode-remote-ssh` jobs in `ci.yml` and `release.yml` drive the CLI,
      Python and the VS Code extension over real SSH and Remote-SSH, and
      `release.yml` publishes nothing unless both pass on the tagged artifacts.
- [ ] Optional: open a fixture in Cursor over Remote-SSH and confirm an image
      appears. Cursor's remote extension cannot run in CI, so this is the only
      check of its port forwarding.
- [ ] Confirm `git status --short` is clean and review the final diff from the
      previous stable release.
- [ ] Commit each remaining logical change according to the repository commit
      policy.

## 5. Push And Qualify The Release-Candidate Commit

- [ ] Record the exact candidate commit:

  ```bash
  RC_SHA="$(git rev-parse HEAD)"
  git show --no-patch --format=fuller "$RC_SHA"
  ```

- [ ] Push the final release-candidate commit through the normal protected-branch
      or pull-request process so it becomes the intended commit on `main`.
- [ ] Monitor every required job in `.github/workflows/ci.yml` to completion.
- [ ] Resolve code, test, packaging, documentation, and deterministic tooling
      failures in the repository. Commit and push each fix, then treat the new
      `HEAD` as a new release candidate and restart this section.
- [ ] For an external blocker that cannot be fixed from the codebase, give the
      maintainer the failing workflow/job URL, affected release channel,
      relevant error excerpt, actions already attempted, release impact, and a
      concrete recommended resolution. Do not tag while a required gate is
      unresolved.
- [ ] After CI passes, verify the remote branch still points at the candidate:

  ```bash
  git fetch origin main
  test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
  ```

## 6. Tag The Passing Commit And Monitor Publication

- [ ] Create an annotated tag on the exact CI-passing commit:

  ```bash
  VERSION="$(python scripts/check_versions.py --print-version)"
  git tag -a "v${VERSION}" "$(git rev-parse HEAD)" -m "dcmview v${VERSION}"
  git show --no-patch "v${VERSION}"
  ```

- [ ] Push only that tag. `main` must already contain the tagged commit:

  ```bash
  VERSION="$(python scripts/check_versions.py --print-version)"
  git push origin "v${VERSION}"
  ```

- [ ] Monitor every required job in `.github/workflows/release.yml`, including
      native builds, archive and wheel smoke tests, VSIX packaging, the remote
      SSH and VS Code Remote-SSH gates, GitHub Release creation, and each enabled PyPI, Open VSX, and Homebrew publisher.
- [ ] Monitor the Azure VS Code Marketplace pipeline and its approval-bound
      deployment.
- [ ] Apply the prepared notes to the GitHub Release and verify its files,
      checksums, version, links, and non-draft/non-prerelease status.
- [ ] Install or download representative published artifacts rather than relying
      only on build-job outputs. Confirm the CLI version and one viewer launch.
- [ ] Verify the live PyPI, VS Code Marketplace, Open VSX, Homebrew, and GitHub
      pages for every enabled channel.

If a workflow fails because of transient infrastructure, credentials, an
approval gate, or an idempotently retryable publisher, preserve the tag and
retry the failed operation after resolving the external condition. If the
tagged code or packaged contents are defective, do not move the tag: fix the
problem on `main` and publish a patch release.

## 7. Synchronize `dcmview-docs` Through A Pull Request

- [ ] Confirm GitHub identifies the new release as the latest stable
      non-draft, non-prerelease release.
- [ ] In `dcmview-docs`, create a release-synchronization branch from current
      `main`.
- [ ] Update every field in `docs-source.json` to the released tag, exact commit,
      and synchronization time in the same pull request.
- [ ] Update release notes and every affected getting-started, guide, concept,
      reference, compatibility, API, and troubleshooting page. Describe only
      behavior present in the recorded release commit.
- [ ] Copy the approved release media and attribution records; do not copy the
      ignored DICOM source directory or its local-path linkage document.
- [ ] Run the documentation repository's required checks:

  ```bash
  npm run format
  npm run validate
  ```

- [ ] Push the branch and open a pull request. Review the Cloudflare preview,
      internal links, media loading, responsive layout, release version, source
      metadata, attribution, and safety language before merge.
- [ ] Merge only after required checks and review pass, then verify the live site.

## 8. Resolve Or Escalate Post-Release Problems

- [ ] Classify each problem as code/package content, release automation,
      credentials/approval, registry availability, documentation, or an
      external service issue.
- [ ] Fix repository-controlled problems with normal commits and tests. Use a
      patch release for changes to already-tagged code or artifacts.
- [ ] For issues outside repository control, clearly flag them to the maintainer
      with the affected channel, severity, evidence, user impact, owner or
      service involved, recommended action, and whether unaffected channels may
      remain available.
- [ ] Record any intentionally deferred issue in release notes or public status
      messaging when users could encounter it.

## 9. Start The Next Development Version

Complete this only after all required publication and documentation checks pass
or every remaining external issue has an explicit maintainer-owned resolution.

- [ ] Choose the next working version. Default to the next patch version unless
      the planned development scope requires a minor or major increment.
- [ ] Update the canonical manifests and regenerate their lockfiles. At minimum,
      keep `Cargo.toml`, `crates/*/Cargo.toml`, `Cargo.lock`, `pyproject.toml`,
      `frontend/package.json`, `frontend/package-lock.json`,
      `vscode/package.json`, and `vscode/package-lock.json` consistent.
- [ ] Leave the released notes under their dated version sections and retain a
      new empty **Unreleased** section in both changelogs.
- [ ] Do not advance `dcmview-docs`; it must continue to describe the latest
      stable release rather than the new development version.
- [ ] Run the package-version check and the appropriate core checks.
- [ ] Commit the version transition separately, for example:

  ```text
  chore(release): begin <next-version> development
  ```

- [ ] Push the version-transition commit to `main` through the normal branch
      policy and verify CI. This commit is the unambiguous starting point for
      the next development cycle.

## Reference: Channels And Automation

Release automation spans two GitHub Actions workflows and one Azure pipeline:

- `.github/workflows/ci.yml` runs frontend, Rust, Python, packaging, and VS Code
  checks on Linux, with Rust coverage on macOS and Windows
- `.github/workflows/release.yml` builds tagged release artifacts for Linux,
  macOS Intel, macOS Apple Silicon, and Windows x64, runs the remote SSH and
  VS Code Remote-SSH checks on the Linux wheel and VSIX, then publishes approved
  releases to PyPI, Homebrew, and Open VSX when configured
- `azure-pipelines/vscode-marketplace.yml` publishes VS Code Marketplace
  packages from GitHub Release assets

### Release channels

- **GitHub Releases** are the canonical binary artifacts
- **PyPI wheels** are the preferred Python install path when `PUBLISH_PYPI=1`
- **VSIX files** are target-specific editor packages attached to GitHub Releases,
  published to the VS Code Marketplace by Azure Pipelines, and published to Open
  VSX for Cursor by GitHub Actions
- **Homebrew** formula generation is always part of the release job; publication
  to a separate tap is conditional on `HOMEBREW_TAP_REPOSITORY`

### Release toolchain

Local release checks and CI require Rust 1.88+, Node.js 20.19+ with npm, and
Python 3.9+. Packaging jobs use Python 3.11. CI pins Rust 1.88 for compatibility
checks, while tagged native builds and the manylinux container install the
current stable Rust toolchain.

### Required repository configuration

Optional release publishing is gated behind repository settings:

- `PUBLISH_PYPI=1` enables the PyPI publish job
- `PUBLISH_OPEN_VSX=1` enables the Open VSX publish job; this must be a
  repository-level Actions variable because the job condition is evaluated
  before GitHub declares its environment
- `HOMEBREW_TAP_REPOSITORY` points to the separate tap repo, for example `your-org/homebrew-tap`
- `HOMEBREW_TAP_TOKEN` is a token with push access to the tap repo

For PyPI, prefer GitHub trusted publishing on the `pypi` environment. The workflow already requests `id-token: write`.

VS Code Marketplace publishing is handled in Azure DevOps:

- Azure DevOps organization: `beatricebm`
- Azure DevOps project: `dcmview`
- Visual Studio Marketplace publisher: `beatricebm`
- Service connection: `dcmview-marketplace-publisher`
- Approval environment: `vscode-marketplace`

The Azure pipeline uses Microsoft Entra ID with workload identity federation and
publishes only VSIX assets that already exist on the GitHub Release.

Open VSX publishing is handled by the `publish-open-vsx` job in GitHub Actions:

- Open VSX namespace: `beatricebm`
- GitHub environment: `open-vsx`
- Environment secret: `OPEN_VSX_PAT`
- Repository variable: `PUBLISH_OPEN_VSX=1`

The environment should require maintainer approval. After approval, the job
downloads the target-specific `vscode-vsix` artifact from the same workflow run
and publishes each platform package with the pinned `ovsx` CLI. The workflow
passes `OPEN_VSX_PAT` to the CLI as `OVSX_PAT` and uses `--skip-duplicate` so a
partially completed release can be retried safely. Generate a dedicated CI token
from the Open VSX account settings, store it only in the protected environment,
and rotate or revoke it if its exposure is suspected.

### What The Release Workflow Does

- build `dcmview` on Ubuntu 22.04, macOS Intel, macOS Apple Silicon, and
  Windows x64
- fail before release builds if the pushed tag does not match the checked-in package versions
- build the Linux PyPI wheel inside a `manylinux_2_28_x86_64` container so the published wheel is PyPI-compatible
- smoke test each built binary against the committed fixture corpus
- validate the Linux release artifact on Ubuntu 22.04 and Ubuntu 24.04
- validate the Windows zip artifact on Windows latest
- build bundled `dcmview-py` wheels
- package target-specific VSIX artifacts for Linux x64, macOS x64, macOS
  arm64, and Windows x64
- publish release tarballs, the Windows zip, checksums, and wheels to GitHub
  Releases
- publish the VSIX artifacts to GitHub Releases
- render `packaging/homebrew/dcmview.rb`
- optionally publish to PyPI, Open VSX, and the configured tap repo
- trigger the Azure pipeline, which waits for the GitHub Release VSIX assets and
  publishes them to the VS Code Marketplace after `vscode-marketplace` approval

### Editor marketplace packages

The VSIX packaging job downloads the same platform archives produced by the
release build matrix and runs:

```bash
npm --prefix vscode ci
npm --prefix vscode run package:release
```

`package:release` builds these target-specific VSIX artifacts:

- `dist/dcmview-<version>-linux-x64.vsix`
- `dist/dcmview-<version>-darwin-x64.vsix`
- `dist/dcmview-<version>-darwin-arm64.vsix`
- `dist/dcmview-<version>-win32-x64.vsix`

Each package contains exactly one bundled binary at
`vscode/resources/bin/<target>/dcmview`, except Windows x64, which contains
`vscode/resources/bin/win32-x64/dcmview.exe`. `dcmview.binaryPath` remains the
override for unsupported platforms, local debug binaries, and troubleshooting
bundled-binary issues.

The Azure Marketplace pipeline is tag-triggered, but the publish deployment is
bound to the `vscode-marketplace` environment. Its approval check provides the
final manual gate without requiring a separate manually triggered release flow.

The GitHub Open VSX job runs only when the repository-level
`PUBLISH_OPEN_VSX` variable equals `1`. It is independently bound to the
`open-vsx` environment, so its required-review rule gates publication without
coupling Cursor availability to the Azure deployment. After a successful first
publication, confirm that Open VSX lists all four target platforms and that
Cursor can find `beatricebm.dcmview`; Cursor's additional security scan may
delay marketplace visibility.

### Homebrew tap publication checklist

Every tagged release renders a dual-architecture macOS formula and attaches it
to the GitHub Release. The separate tap publication job runs only when
`HOMEBREW_TAP_REPOSITORY` is configured. Do not add public install commands to
the README or release notes until the named tap exists and has published a
working formula.

Before the first release intended for a public tap:

- Create or select the Homebrew tap repository that will receive
  `Formula/dcmview.rb`.
- Set the repository variable `HOMEBREW_TAP_REPOSITORY` to the tap repository in
  `owner/repo` form.
- Add the repository secret `HOMEBREW_TAP_TOKEN` with push access to the tap
  repository.
- Confirm the tap repository accepts commits from GitHub Actions and does not
  require branch protection rules that the release workflow cannot satisfy.
- Confirm the generated formula artifact from a dry run or previous release
  contains both macOS archive URLs and SHA-256 checksums.

During that release:

- Verify the `Render Homebrew formula` step uploads the `homebrew-formula`
  artifact.
- Verify the `publish-homebrew-tap` job runs when `HOMEBREW_TAP_REPOSITORY` is
  set and commits `Formula/dcmview.rb` to the tap.
- Run `brew audit --strict --online dcmview` and `brew test dcmview` from the
  tap repository after publication.
- Add public Homebrew install commands only after the tap contains a working
  formula for the tagged release.

## Completion Record

Record these values in the release issue, pull request, or maintainer log:

```text
Previous stable tag:
Release tag:
Release commit:
CI workflow URL:
Release workflow URL:
GitHub Release URL:
PyPI result:
VS Code Marketplace result:
Open VSX result:
Homebrew result:
dcmview-docs PR and preview URL:
Media manifest/hash:
Known issues or external follow-ups:
Next development version commit:
```
