# Windows release signing

Windows tag builds use one Authenticode identity for the complete install chain:

1. `sister.exe`
2. `sister-desktop.exe`
3. NSIS `uninstall.exe`
4. `AI-Sister-Setup.exe`

The signature digest is SHA-256. Production signatures use the fixed RFC 3161 timestamp service
`http://timestamp.digicert.com`. The release job publishes only after the Windows job verifies all
four layers, then binds the three public files to one receipt containing their exact size, SHA-256,
publisher, certificate thumbprint and timestamp signer.

## Certificate

Use one public-CA PFX containing exactly one current Code Signing certificate with its private key.
The certificate must have the Code Signing EKU (`1.3.6.1.5.5.7.3.3`) and a trusted chain. Production
builds reject self-signed certificates.

Store the PFX and password as GitHub Actions encrypted secrets:

- `AI_SISTER_WINDOWS_PFX_BASE64`
- `AI_SISTER_WINDOWS_PFX_PASSWORD`

PowerShell:

```powershell
[Convert]::ToBase64String(
  [IO.File]::ReadAllBytes((Resolve-Path .\ai-sister-code-signing.pfx))
) | gh secret set AI_SISTER_WINDOWS_PFX_BASE64
gh secret set AI_SISTER_WINDOWS_PFX_PASSWORD
```

Bash:

```bash
base64 -w 0 ./ai-sister-code-signing.pfx | gh secret set AI_SISTER_WINDOWS_PFX_BASE64
gh secret set AI_SISTER_WINDOWS_PFX_PASSWORD
```

The PFX is decoded only on the Windows tag runner, imported into the current user's certificate
store, deleted before the build continues, and removed from the store in the final cleanup step.
Branches never import the release identity.

## Publication policy

- A prerelease tag is signed when both secrets are present. Without them it is verified and
  published as unsigned.
- A stable tag such as `v1.0.0` is rejected before build when either secret is absent.
- A partial secret configuration is always rejected.
- A stable release receipt must report production mode, one publisher and one certificate across
  all three public files, plus an RFC 3161 timestamp signer on every file.
- The receipt is a CI artifact. It is verified before the draft release is created and is not a
  public release asset.

Unsigned branch and prerelease runs still exercise the complete Tauri signing path with an isolated,
throwaway trusted certificate after the public files have been staged. CI rebuilds, installs and
verifies the signed main executable, sidecar, uninstaller and Setup, then removes the fixture.

## Local verification

Run both checks on each downloaded executable:

```powershell
Get-AuthenticodeSignature .\AI-Sister-Setup.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
signtool verify /pa /all /v .\AI-Sister-Setup.exe
```

`Status` must be `Valid`; the publisher must match the release identity and
`TimeStamperCertificate` must be present.
