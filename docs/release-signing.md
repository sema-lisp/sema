# Release binary trust: signing, notarization, attestations

Release macOS binaries are Developer-ID-signed during `dist build` and
notarized before the release is published. Everything is automatic
in CI once the six repo secrets below exist; missing signing or notarization credentials fail the release gate. Independently of the
secrets, every release binary archive gets a GitHub Artifact Attestation
(issue #107) — see the last section.

## How it's wired

- **Signing** — `macos-sign = true` in `dist-workspace.toml` turns on dist's
  built-in codesigning: during `build-local-artifacts` on the two darwin
  runners, dist imports the certificate into an ephemeral keychain and signs
  the `sema` binary *before* archiving, so all downstream checksums (`.sha256`,
  `dist-manifest.json`, Homebrew formula) are computed over the signed binary.
  `.github/build-setup.yml` exports `CODESIGN_OPTIONS=runtime` (hardened
  runtime), which notarization requires; the secure timestamp it also requires
  is added by `codesign` automatically for Developer ID identities (verified
  empirically — `codesign -dvv` shows `Timestamp=` even without `--timestamp`).
- **Notarization** — the `check-macos-artifacts` global-artifact job submits
  both signed architecture binaries and the universal MCP executable to Apple
  before cargo-dist can publish. It requires an Accepted result from notarytool.
  ARM64 and Intel runners then verify signatures, dependencies, version and MCP
  operation, and require `codesign --check-notarization -R=notarized` on their
  native and universal executables. They also set the downloaded-file quarantine
  attribute before launch and MCP checks. `spctl --type execute` is an app-bundle
  assessment and rejects valid standalone executables as "not an app".
  Bare executables cannot be stapled; Apple registers their code hashes online.
  Submission does not modify the archives or their checksums.
- **Preflight** — `validate-release.yml` runs on release-branch pushes and manual
  dispatch. It uses `dist plan` and the generated release build job, then the
  same notarization and launch gate. It creates no tag, release, registry entry,
  or Homebrew commit. After `dist generate`, run
  `python3 scripts/generate-release-validation.py` to refresh that job.
- `sema build` standalone executables are unaffected: libsui re-signs its
  output ad-hoc after embedding the archive, same as today.

## One-time setup (six repo secrets)

### 1. Signing certificate (three `CODESIGN_*` secrets)

Export the **Developer ID Application** certificate *with its private key*
from Keychain Access (on the machine that has it — currently
`Developer ID Application: Liseth Solutions AS (9Z2L5FBZS3)`):
Keychain Access → My Certificates → right-click the cert → Export → `.p12`,
choose an export password.

```bash
gh secret set CODESIGN_CERTIFICATE          --repo sema-lisp/sema --body "$(base64 -i cert.p12)"
gh secret set CODESIGN_CERTIFICATE_PASSWORD --repo sema-lisp/sema --body '<the .p12 export password>'
gh secret set CODESIGN_IDENTITY             --repo sema-lisp/sema --body 'Developer ID Application: Liseth Solutions AS (9Z2L5FBZS3)'
```

`CODESIGN_IDENTITY` must match the certificate's common name exactly
(`security find-identity -v -p codesigning` shows it).

### 2. Notary credentials (three `APPLE_API_*` secrets)

Create an **App Store Connect API key**: appstoreconnect.apple.com → Users and
Access → Integrations → App Store Connect API → Team Keys → Generate. Role:
Developer is sufficient. Download the `.p8` (downloadable **once**), note the
Key ID and the page's Issuer ID.

```bash
gh secret set APPLE_API_KEY_P8    --repo sema-lisp/sema --body "$(cat AuthKey_XXXXXXXXXX.p8)"
gh secret set APPLE_API_KEY_ID    --repo sema-lisp/sema --body '<key id>'
gh secret set APPLE_API_ISSUER_ID --repo sema-lisp/sema --body '<issuer uuid>'
```

## Required credentials

All six signing and notarization secrets are required. Missing credentials fail
validation before publication; an unsigned build is not a release candidate.

## Verifying a release

```bash
# flags=0x10000(runtime), Authority=Developer ID Application: …, Timestamp=…
codesign --display --verbose sema
# Notarize job log in the Release workflow run, or:
xcrun notarytool history --key AuthKey.p8 --key-id <id> --issuer <issuer>
```

End-to-end Gatekeeper check: download a `.tar.xz` asset **via a browser** (so
it gets quarantined), extract, run — it must start without the "unidentified
developer" dialog.

## Renewal

Developer ID Application certificates last 5 years. On expiry: new cert in
Keychain Access, re-export, update `CODESIGN_CERTIFICATE` (+ password/identity
if changed). The API key doesn't expire unless revoked.

## Provenance: GitHub Artifact Attestations

`github-attestations = true` in `dist-workspace.toml` (issue #107) makes the
build jobs publish Sigstore provenance for each per-target binary archive —
keyless, signed with the workflow's OIDC identity, no secrets involved. The
`.sha256` sidecars only guard against corruption (they ship over the same
channel as the binary); an attestation proves the artifact was built by this
repo's Release workflow at a specific commit. Verify a download with:

```bash
gh attestation verify sema-lang-aarch64-apple-darwin.tar.xz --owner sema-lisp
```

Scope note: dist 0.30.4 attests the binary archives from `build-local-artifacts`
only — installers (`.sh`/`.ps1`), the Homebrew formula and the source tarball
are not attested.

## macOS dependency and launch gate

The CLI statically links the bundled liblzma through `lzma-sys/static`. Do not
remove that feature: pkg-config can otherwise select a Homebrew dylib from the
build machine, which hardened-runtime library validation rejects (#163).

`check-macos-artifacts.yml` is a cargo-dist global-artifact job. Before the
release is published it extracts the signed archives on ARM64 and Intel runners,
rejects non-system dynamic dependencies, verifies signatures, and runs CLI and
MCP initialization checks. It also checks a universal executable assembled from
both slices, as used by `sema.mcpb`. `pack-mcpb.sh` repeats that check on the exact
universal executable placed in a release bundle. No check re-signs the binary.

Run the same check on an extracted release binary on its native architecture:

```sh
python3 scripts/check-macos-release.py /path/to/sema 1.36.1
```

Signature verification alone does not prove notarization. The gate separately
requires Apple acceptance, the explicit `notarized` requirement, and quarantined
launch checks before publication. This follows Apple’s [verification guidance
for non-app code](https://developer.apple.com/videos/play/wwdc2019/703/?time=907).
