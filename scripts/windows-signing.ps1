[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('Prepare', 'PrepareSelfTest', 'SignExpected', 'VerifyExpected', 'Receipt', 'Cleanup')]
  [string] $Action,

  [string[]] $Path = @(),
  [string] $PlanPath = '',
  [string] $ReceiptPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$timestampUrl = 'http://timestamp.digicert.com'
$codeSigningEku = '1.3.6.1.5.5.7.3.3'

function Write-Utf8Json([string] $Destination, [object] $Value) {
  $parent = Split-Path -Parent $Destination
  if (-not [string]::IsNullOrEmpty($parent)) {
    $null = New-Item -ItemType Directory -Path $parent -Force
  }
  $json = $Value | ConvertTo-Json -Depth 10
  [IO.File]::WriteAllText(
    $Destination,
    $json + [Environment]::NewLine,
    [Text.UTF8Encoding]::new($false)
  )
}

function Read-Plan([string] $Source) {
  if ([string]::IsNullOrWhiteSpace($Source) -or
      -not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "Windows signing plan 不在：$Source"
  }
  return Get-Content -LiteralPath $Source -Raw -Encoding UTF8 | ConvertFrom-Json
}

function Add-GitHubEnvironment([string] $Name, [string] $Value) {
  if ([string]::IsNullOrWhiteSpace($env:GITHUB_ENV)) {
    throw 'GITHUB_ENV 不在，無法把簽章計畫交給後續 build step'
  }
  if ($Name -notmatch '^[A-Z0-9_]+$' -or $Value.Contains("`r") -or $Value.Contains("`n")) {
    throw "不能寫入 GitHub environment：$Name"
  }
  Add-Content -LiteralPath $env:GITHUB_ENV -Encoding UTF8 -Value "$Name=$Value"
}

function Find-SignTool {
  $command = Get-Command 'signtool.exe' -ErrorAction SilentlyContinue
  if ($null -ne $command) {
    return $command.Source
  }

  $programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
  $sdkBin = Join-Path $programFilesX86 'Windows Kits\10\bin'
  $matches = @(
    Get-ChildItem -LiteralPath $sdkBin -Directory -ErrorAction SilentlyContinue |
      Sort-Object Name -Descending |
      ForEach-Object {
        $candidate = Join-Path $_.FullName 'x64\signtool.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
          Get-Item -LiteralPath $candidate
        }
      }
  )
  if ($matches.Count -eq 0) {
    throw 'Windows SDK signtool.exe 不在'
  }
  return $matches[0].FullName
}

function Normalize-Thumbprint([string] $Value) {
  return ($Value -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
}

function Has-CodeSigningEku($Certificate) {
  return @(
    $Certificate.EnhancedKeyUsageList |
      Where-Object { $_.ObjectId -ceq $codeSigningEku }
  ).Count -gt 0
}

function Get-Certificate([string] $Thumbprint, [string] $Store = 'My') {
  $normalized = Normalize-Thumbprint $Thumbprint
  if ($normalized.Length -ne 40) {
    throw "簽章 certificate thumbprint 不是 40 位 SHA-1：$normalized"
  }
  $certificatePath = "Cert:\CurrentUser\$Store\$normalized"
  if (-not (Test-Path -LiteralPath $certificatePath -PathType Leaf)) {
    throw "簽章 certificate 不在 CurrentUser/$Store：$normalized"
  }
  return Get-Item -LiteralPath $certificatePath
}

function Assert-CertificateUsable($Certificate, [switch] $AllowSelfSigned) {
  $now = Get-Date
  if (-not $Certificate.HasPrivateKey) {
    throw '簽章 certificate 沒有 private key'
  }
  if (-not (Has-CodeSigningEku $Certificate)) {
    throw '簽章 certificate 沒有 Code Signing EKU'
  }
  if ($Certificate.NotBefore -gt $now -or $Certificate.NotAfter -le $now) {
    throw '簽章 certificate 不在有效期間'
  }
  if (-not $AllowSelfSigned -and $Certificate.Subject -ceq $Certificate.Issuer) {
    throw '正式 Windows release 不接受 self-signed certificate'
  }
}

function Get-FileProjection([string] $InputPath, $Plan) {
  $resolved = (Resolve-Path -LiteralPath $InputPath).Path
  $item = Get-Item -LiteralPath $resolved
  if ($item.Extension -cne '.exe') {
    throw "Windows 簽章只接受 exe：$resolved"
  }

  $signature = Get-AuthenticodeSignature -LiteralPath $resolved
  $expectedMode = [string] $Plan.mode
  if ($expectedMode -ceq 'unsigned') {
    if ($signature.Status.ToString() -cne 'NotSigned' -or
        $null -ne $signature.SignerCertificate -or
        $null -ne $signature.TimeStamperCertificate) {
      throw "unsigned build 出現非預期 Authenticode：$resolved status=$($signature.Status)"
    }
    $signatureProjection = [ordered]@{
      state = 'unsigned'
      publisher = $null
      certificate_thumbprint = $null
      timestamp_publisher = $null
    }
  }
  elseif ($expectedMode -in @('production', 'fixture')) {
    $expectedThumbprint = Normalize-Thumbprint ([string] $Plan.certificate_thumbprint)
    if ($signature.Status.ToString() -cne 'Valid') {
      throw "Authenticode 驗證失敗：$resolved status=$($signature.Status) message=$($signature.StatusMessage)"
    }
    if ($null -eq $signature.SignerCertificate) {
      throw "Authenticode 沒有 signer certificate：$resolved"
    }
    $actualThumbprint = Normalize-Thumbprint $signature.SignerCertificate.Thumbprint
    if ($actualThumbprint -cne $expectedThumbprint) {
      throw "Authenticode signer 不符：$resolved expected=$expectedThumbprint actual=$actualThumbprint"
    }
    if ($expectedMode -ceq 'production' -and $null -eq $signature.TimeStamperCertificate) {
      throw "正式 Authenticode 沒有 RFC 3161 timestamp：$resolved"
    }

    $signTool = Find-SignTool
    & $signTool verify /pa /all /v $resolved
    if ($LASTEXITCODE -ne 0) {
      throw "signtool verify 失敗：$resolved exit=$LASTEXITCODE"
    }

    $timestampPublisher = $null
    if ($null -ne $signature.TimeStamperCertificate) {
      $timestampPublisher = $signature.TimeStamperCertificate.Subject
    }
    $signatureProjection = [ordered]@{
      state = if ($expectedMode -ceq 'production') { 'trusted-rfc3161' } else { 'trusted-fixture' }
      publisher = $signature.SignerCertificate.Subject
      certificate_thumbprint = $actualThumbprint.ToLowerInvariant()
      timestamp_publisher = $timestampPublisher
    }
  }
  else {
    throw "未知 Windows signing mode：$expectedMode"
  }

  return [ordered]@{
    name = $item.Name
    bytes = $item.Length
    sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
    signature = $signatureProjection
  }
}

function Sign-File([string] $InputPath, $Plan) {
  if ([string] $Plan.mode -cne 'production') {
    $null = Get-FileProjection $InputPath $Plan
    return
  }

  $resolved = (Resolve-Path -LiteralPath $InputPath).Path
  $before = Get-AuthenticodeSignature -LiteralPath $resolved
  if ($before.Status.ToString() -cne 'NotSigned') {
    throw "簽章入口只接受尚未簽署的 exe：$resolved status=$($before.Status)"
  }

  $thumbprint = Normalize-Thumbprint ([string] $Plan.certificate_thumbprint)
  $null = Get-Certificate $thumbprint
  $signTool = Find-SignTool
  & $signTool sign /fd SHA256 /sha1 $thumbprint /d AI-Sister /tr $timestampUrl /td SHA256 $resolved
  if ($LASTEXITCODE -ne 0) {
    throw "signtool sign 失敗：$resolved exit=$LASTEXITCODE"
  }
  $null = Get-FileProjection $resolved $Plan
}

function Prepare-Production([string] $Destination) {
  $pfxBase64 = [Environment]::GetEnvironmentVariable('AI_SISTER_WINDOWS_PFX_BASE64')
  $pfxPassword = [Environment]::GetEnvironmentVariable('AI_SISTER_WINDOWS_PFX_PASSWORD')
  $pfxPresent = -not [string]::IsNullOrWhiteSpace($pfxBase64)
  $passwordPresent = -not [string]::IsNullOrWhiteSpace($pfxPassword)

  $plannerArguments = @(
    (Join-Path $PSScriptRoot 'windows-signing-plan.py'),
    '--ref-type', [string] $env:GITHUB_REF_TYPE,
    '--ref-name', [string] $env:GITHUB_REF_NAME
  )
  if ($pfxPresent) { $plannerArguments += '--pfx-present' }
  if ($passwordPresent) { $plannerArguments += '--password-present' }
  $planText = & python @plannerArguments
  if ($LASTEXITCODE -ne 0) {
    throw 'Windows signing policy 拒絕這一輪 build'
  }
  $policy = $planText | ConvertFrom-Json

  if ([string] $policy.mode -ceq 'unsigned') {
    $plan = [ordered]@{
      schema = 1
      mode = 'unsigned'
      stable_release = [bool] $policy.stable_release
      ref_type = [string] $policy.ref_type
      ref_name = [string] $policy.ref_name
      digest_algorithm = $null
      timestamp_url = $null
      certificate_subject = $null
      certificate_thumbprint = $null
      remove_from = @()
    }
    Write-Utf8Json $Destination $plan
    Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_MODE' 'unsigned'
    Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_PLAN' $Destination
    return
  }

  $rawPfx = $null
  $imported = @()
  $pfxPath = Join-Path $env:RUNNER_TEMP 'ai-sister-windows-signing.pfx'
  try {
    $rawPfx = [Convert]::FromBase64String($pfxBase64)
    [IO.File]::WriteAllBytes($pfxPath, $rawPfx)
    $securePassword = ConvertTo-SecureString -String $pfxPassword -AsPlainText -Force
    $imported = @(
      Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation 'Cert:\CurrentUser\My' `
        -Password $securePassword -Exportable:$false
    )
  }
  finally {
    if ($null -ne $rawPfx) {
      [Array]::Clear($rawPfx, 0, $rawPfx.Length)
    }
    if (Test-Path -LiteralPath $pfxPath) {
      Remove-Item -LiteralPath $pfxPath -Force
    }
  }

  $candidates = @(
    $imported | Where-Object { $_.HasPrivateKey -and (Has-CodeSigningEku $_) }
  )
  if ($candidates.Count -ne 1) {
    throw "PFX 內應恰有一張帶 private key 的 Code Signing certificate，實際：$($candidates.Count)"
  }
  $certificate = $candidates[0]
  Assert-CertificateUsable $certificate
  $thumbprint = Normalize-Thumbprint $certificate.Thumbprint

  $configPath = Join-Path $env:RUNNER_TEMP 'ai-sister-tauri-production-signing.json'
  $config = [ordered]@{
    bundle = [ordered]@{
      windows = [ordered]@{
        certificateThumbprint = $thumbprint
        digestAlgorithm = 'sha256'
        timestampUrl = $timestampUrl
        tsp = $true
      }
    }
  }
  Write-Utf8Json $configPath $config

  $plan = [ordered]@{
    schema = 1
    mode = 'production'
    stable_release = [bool] $policy.stable_release
    ref_type = [string] $policy.ref_type
    ref_name = [string] $policy.ref_name
    digest_algorithm = 'sha256'
    timestamp_url = $timestampUrl
    certificate_subject = $certificate.Subject
    certificate_thumbprint = $thumbprint.ToLowerInvariant()
    remove_from = @('My')
  }
  Write-Utf8Json $Destination $plan
  Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_MODE' 'production'
  Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_PLAN' $Destination
  Add-GitHubEnvironment 'AI_SISTER_WINDOWS_TAURI_SIGNING_CONFIG' $configPath
}

function Prepare-SelfTest([string] $Destination) {
  $subject = "CN=AI-Sister CI signing fixture $([Guid]::NewGuid().ToString('N'))"
  Write-Host 'signing fixture: create certificate'
  $certificate = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $subject `
    -CertStoreLocation 'Cert:\CurrentUser\My' `
    -KeyAlgorithm RSA `
    -KeyLength 2048 `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy Exportable `
    -NotAfter (Get-Date).AddDays(2)
  Assert-CertificateUsable $certificate -AllowSelfSigned

  $publicCertificate = Join-Path $env:RUNNER_TEMP 'ai-sister-signing-fixture.cer'
  $fixturePfx = Join-Path $env:RUNNER_TEMP 'ai-sister-signing-fixture.pfx'
  $fixturePasswordText = [Guid]::NewGuid().ToString('N')
  $fixturePassword = ConvertTo-SecureString -String $fixturePasswordText -AsPlainText -Force
  $thumbprint = Normalize-Thumbprint $certificate.Thumbprint
  try {
    Write-Host 'signing fixture: export public certificate and PFX'
    $null = Export-Certificate -Cert $certificate -FilePath $publicCertificate -Force
    $null = Export-PfxCertificate `
      -Cert $certificate `
      -FilePath $fixturePfx `
      -Password $fixturePassword `
      -ChainOption EndEntityCertOnly `
      -Force

    # 隔離演練不是直接沿用剛建立的 certificate。先移除，再走和正式 release
    # 相同的 PFX -> CurrentUser/My import，才能抓到 PFX 密碼、private key 與 store 接線。
    Write-Host 'signing fixture: remove source certificate and import PFX'
    Remove-Item -LiteralPath "Cert:\CurrentUser\My\$thumbprint" -Force
    $imported = @(
      Import-PfxCertificate -FilePath $fixturePfx -CertStoreLocation 'Cert:\CurrentUser\My' `
        -Password $fixturePassword -Exportable:$false
    )
    $candidates = @(
      $imported | Where-Object { $_.HasPrivateKey -and (Has-CodeSigningEku $_) }
    )
    if ($candidates.Count -ne 1) {
      throw "fixture PFX 內應恰有一張帶 private key 的 Code Signing certificate，實際：$($candidates.Count)"
    }
    $certificate = $candidates[0]
    Assert-CertificateUsable $certificate -AllowSelfSigned
    if ((Normalize-Thumbprint $certificate.Thumbprint) -cne $thumbprint) {
      throw 'fixture PFX round-trip 改變了 certificate thumbprint'
    }
    # Certificate Provider 對 Root store 的互動模式受 host 影響；certutil 的 user + force
    # 路徑是明確非互動，並且不需要提升到 LocalMachine。
    Write-Host 'signing fixture: trust public certificate for current user'
    & certutil.exe -user -f -silent -addstore Root $publicCertificate
    if ($LASTEXITCODE -ne 0) {
      throw "fixture Root trust import 失敗：certutil exit=$LASTEXITCODE"
    }
    Write-Host 'signing fixture: certificate round-trip complete'
  }
  finally {
    if (Test-Path -LiteralPath $publicCertificate) {
      Remove-Item -LiteralPath $publicCertificate -Force
    }
    if (Test-Path -LiteralPath $fixturePfx) {
      Remove-Item -LiteralPath $fixturePfx -Force
    }
    $fixturePasswordText = $null
  }

  $configPath = Join-Path $env:RUNNER_TEMP 'ai-sister-tauri-signing-fixture.json'
  $config = [ordered]@{
    bundle = [ordered]@{
      windows = [ordered]@{
        certificateThumbprint = $thumbprint
        digestAlgorithm = 'sha256'
      }
    }
  }
  Write-Utf8Json $configPath $config

  $plan = [ordered]@{
    schema = 1
    mode = 'fixture'
    stable_release = $false
    ref_type = 'ci-fixture'
    ref_name = 'ci-fixture'
    digest_algorithm = 'sha256'
    timestamp_url = $null
    certificate_subject = $certificate.Subject
    certificate_thumbprint = $thumbprint.ToLowerInvariant()
    remove_from = @('My', 'Root')
  }
  Write-Utf8Json $Destination $plan
  Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_SELF_TEST_PLAN' $Destination
  Add-GitHubEnvironment 'AI_SISTER_WINDOWS_SIGNING_SELF_TEST_CONFIG' $configPath
}

if ($Action -ceq 'Prepare') {
  if ([string]::IsNullOrWhiteSpace($PlanPath)) {
    throw 'Prepare 需要 -PlanPath'
  }
  Prepare-Production $PlanPath
  exit 0
}

if ($Action -ceq 'PrepareSelfTest') {
  if ([string]::IsNullOrWhiteSpace($PlanPath)) {
    throw 'PrepareSelfTest 需要 -PlanPath'
  }
  Prepare-SelfTest $PlanPath
  exit 0
}

$plan = Read-Plan $PlanPath

if ($Action -ceq 'SignExpected') {
  if ($Path.Count -eq 0) { throw 'SignExpected 需要 -Path' }
  foreach ($inputPath in $Path) {
    Sign-File $inputPath $plan
  }
  exit 0
}

if ($Action -ceq 'VerifyExpected') {
  if ($Path.Count -eq 0) { throw 'VerifyExpected 需要 -Path' }
  foreach ($inputPath in $Path) {
    $null = Get-FileProjection $inputPath $plan
  }
  exit 0
}

if ($Action -ceq 'Receipt') {
  if ($Path.Count -eq 0) { throw 'Receipt 需要 -Path' }
  if ([string]::IsNullOrWhiteSpace($ReceiptPath)) { throw 'Receipt 需要 -ReceiptPath' }
  $files = @($Path | ForEach-Object { Get-FileProjection $_ $plan })
  $receipt = [ordered]@{
    schema = 1
    mode = [string] $plan.mode
    stable_release = [bool] $plan.stable_release
    ref_type = [string] $plan.ref_type
    ref_name = [string] $plan.ref_name
    digest_algorithm = $plan.digest_algorithm
    timestamp_url = $plan.timestamp_url
    certificate_subject = $plan.certificate_subject
    certificate_thumbprint = $plan.certificate_thumbprint
    files = $files
  }
  Write-Utf8Json $ReceiptPath $receipt
  exit 0
}

if ($Action -ceq 'Cleanup') {
  $thumbprint = Normalize-Thumbprint ([string] $plan.certificate_thumbprint)
  if (-not [string]::IsNullOrEmpty($thumbprint)) {
    foreach ($store in @($plan.remove_from)) {
      $certificatePath = "Cert:\CurrentUser\$store\$thumbprint"
      if (Test-Path -LiteralPath $certificatePath -PathType Leaf) {
        Remove-Item -LiteralPath $certificatePath -Force
      }
    }
  }
  exit 0
}

throw "未處理的 Windows signing action：$Action"
