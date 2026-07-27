# macOS release signing & notarization

Release macOS binaries are Developer-ID-signed during `dist build` and
notarized after the release is published (issue #109). Everything is automatic
in CI once the six repo secrets below exist; without them, releases keep
working exactly as before (ad-hoc-signed, not notarized).

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
- **Notarization** — dist has no built-in notarization (explicitly "future
  work" in its `sign/macos.rs`), so `post-announce-jobs = ["./notarize"]` runs
  `.github/workflows/notarize.yml` after the GitHub release exists: it
  downloads the two `*-apple-darwin.tar.xz` assets, checks the signature
  actually carries the hardened-runtime flag, zips each binary, and submits it
  with `notarytool --wait`. Bare Mach-O binaries can't be stapled (only
  .app/.dmg/.pkg can), so nothing is re-uploaded and no checksum changes;
  Gatekeeper fetches the notarization ticket from Apple online on first launch
  of a quarantined (browser-downloaded) copy.
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

## Degradation matrix

| Secrets present | Result |
| --- | --- |
| none | Ad-hoc binaries, notarize job skips with a notice — current behavior |
| `CODESIGN_*` only | Signed + hardened runtime; notarize job skips with a notice |
| `APPLE_API_*` only | Notarize job fails loudly at the hardened-runtime guard (binaries unsigned) |
| all six | Signed and notarized |

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
