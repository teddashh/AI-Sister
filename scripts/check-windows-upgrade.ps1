[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $CurrentSetup,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $CurrentSister,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $CurrentDesktopPortable,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $CurrentTauriConfig,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $Scenario,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string] $EvidenceFixture
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest

# PowerShell uses Console.OutputEncoding when it turns redirected native stdout
# and stderr into strings. GitHub's Windows images have not always agreed on
# that process-wide default, while sister and Python deliberately emit UTF-8.
# Pin the console for readable logs and $OutputEncoding for the Python scripts
# piped over stdin. Machine-checked CLI output also goes through the stricter
# per-process decoder in Invoke-NativeUtf8 below.
$utf8NoBom = [Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8NoBom
[Console]::InputEncoding = $utf8NoBom
[Console]::OutputEncoding = $utf8NoBom

# This is deliberately a CI-only destructive smoke: NSIS owns a fixed current-user
# uninstall key, and the product owns a fixed HKCU Run value. Keeping that fact in
# the script prevents somebody from running a "harmless checker" on their daily
# Windows profile and replacing the real installation.
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:OS -cne 'Windows_NT') {
  throw '這支升級閘門只准在 GitHub Actions 的 disposable Windows runner 執行'
}
if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
  throw 'RUNNER_TEMP 未知，不能猜測 installer／外部資料的隔離位置'
}

$baselineVersion = '0.1.0-alpha.110'
$expectedCurrentVersion = '0.1.0-alpha.134'
$baselineUrl = 'https://github.com/teddashh/AI-Sister/releases/download/v0.1.0-alpha.110/AI-Sister-Setup.exe'
[int64] $baselineSetupBytes = 305417570
$baselineSetupSha256 = '3e661803d1b1d867aae0281e56baeb6a165b9178069ad912dc0c8869d1521965'
[int64] $baselineSisterBytes = 10589184
$baselineSisterSha256 = '352aa9977fdc5f653c060a453a132ad1b1f21369242ed1310c2dc4ad4d1b8eca'

$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$loginRunName = 'AI-Sister'
$unrelatedRunName = 'AI-Sister-CI-Upgrade-Unrelated'
$unrelatedRunValue = '"C:\Windows\System32\notepad.exe" --ai-sister-upgrade-unrelated'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\AI-Sister'

function Resolve-RequiredFile([string] $Path, [string] $Label) {
  $resolved = Resolve-Path -LiteralPath $Path -ErrorAction Stop
  if (-not [IO.File]::Exists($resolved.Path)) {
    throw "$Label 不是可讀的檔案：$($resolved.Path)"
  }
  return $resolved.Path
}

function Get-RequiredFileState([string] $Path, [string] $Label) {
  if (-not [IO.File]::Exists($Path)) {
    throw "$Label 不存在，不能用空值或 0 冒充已量到：$Path"
  }
  $file = Get-Item -LiteralPath $Path
  $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
  return [pscustomobject]@{
    Length = [int64] $file.Length
    Sha256 = $hash
  }
}

function Assert-PublishedFile(
  [string] $Path,
  [int64] $ExpectedBytes,
  [string] $ExpectedSha256,
  [string] $Label
) {
  if ($ExpectedBytes -le 0 -or $ExpectedSha256 -notmatch '^[0-9a-f]{64}$') {
    throw "$Label 的 expected bytes/hash 不是已知 authority"
  }
  $actual = Get-RequiredFileState $Path $Label
  if ($actual.Length -ne $ExpectedBytes -or $actual.Sha256 -cne $ExpectedSha256) {
    throw ("{0} 不等於 pinned 公開 artifact：bytes={1} sha256={2}；expected={3}/{4}" -f `
      $Label, $actual.Length, $actual.Sha256, $ExpectedBytes, $ExpectedSha256)
  }
}

function Assert-SameFileState(
  [string] $Path,
  [pscustomobject] $Expected,
  [string] $Failure
) {
  $actual = Get-RequiredFileState $Path $Failure
  if ($actual.Length -ne $Expected.Length -or $actual.Sha256 -cne $Expected.Sha256) {
    throw ("{0}：bytes/hash {1}/{2} -> {3}/{4}" -f `
      $Failure, $Expected.Length, $Expected.Sha256, $actual.Length, $actual.Sha256)
  }
}

function Assert-ExactFile([string] $ActualPath, [string] $ExpectedPath, [string] $Failure) {
  [byte[]] $actual = [IO.File]::ReadAllBytes($ActualPath)
  [byte[]] $expected = [IO.File]::ReadAllBytes($ExpectedPath)
  if ($actual.Length -ne $expected.Length -or
      -not [System.Linq.Enumerable]::SequenceEqual[byte]($actual, $expected)) {
    $actualHash = [Convert]::ToHexString(
      [Security.Cryptography.SHA256]::HashData($actual)).ToLowerInvariant()
    $expectedHash = [Convert]::ToHexString(
      [Security.Cryptography.SHA256]::HashData($expected)).ToLowerInvariant()
    throw "$Failure：actual=$actualHash expected=$expectedHash"
  }
}

function Assert-NsisDesktopPayload([string] $InstalledPath, [string] $PortablePath) {
  # tauri-bundler changes exactly one fixed-width bundle marker from UNK to NSS
  # before embedding the desktop executable. The portable staging binary is put
  # back to UNK afterwards, so a plain hash comparison would reject the correct
  # payload. Derive the exact expected installed bytes instead.
  [byte[]] $expected = [IO.File]::ReadAllBytes($PortablePath)
  $latin1 = [Text.Encoding]::Latin1.GetString($expected)
  $unknown = '__TAURI_BUNDLE_TYPE_VAR_UNK'
  $nsis = '__TAURI_BUNDLE_TYPE_VAR_NSS'
  $marker = $latin1.IndexOf($unknown, [StringComparison]::Ordinal)
  if ($marker -lt 0 -or
      $marker -ne $latin1.LastIndexOf($unknown, [StringComparison]::Ordinal)) {
    throw 'current portable desktop 沒有恰好一個 Tauri UNK bundle marker'
  }
  [byte[]] $replacement = [Text.Encoding]::ASCII.GetBytes($nsis)
  if ($replacement.Length -ne $unknown.Length) {
    throw 'Tauri NSS 與 UNK marker 不再等長，不能推導 exact installed payload'
  }
  [Array]::Copy($replacement, 0, $expected, $marker, $replacement.Length)

  [byte[]] $actual = [IO.File]::ReadAllBytes($InstalledPath)
  if ($actual.Length -ne $expected.Length -or
      -not [System.Linq.Enumerable]::SequenceEqual[byte]($actual, $expected)) {
    $actualHash = [Convert]::ToHexString(
      [Security.Cryptography.SHA256]::HashData($actual)).ToLowerInvariant()
    $expectedHash = [Convert]::ToHexString(
      [Security.Cryptography.SHA256]::HashData($expected)).ToLowerInvariant()
    throw "upgrade 後 desktop 不是 current portable 的 exact NSIS 衍生檔：actual=$actualHash expected=$expectedHash"
  }
}

function Invoke-NativeUtf8 {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $Path,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]] $Arguments,

    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 600)]
    [int] $TimeoutSeconds,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $Label
  )

  # Setting Console.OutputEncoding is necessary for PowerShell's own native
  # pipeline, but this checker must not silently fall back to a runner code page.
  # Decode each captured stream as strict UTF-8 and fail on malformed bytes.
  $strictUtf8 = [Text.UTF8Encoding]::new($false, $true)
  $startInfo = [Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $Path
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $startInfo.StandardOutputEncoding = $strictUtf8
  $startInfo.StandardErrorEncoding = $strictUtf8
  foreach ($argument in $Arguments) {
    if ($null -eq $argument) {
      throw "$Label 的 native argument 不得是 null"
    }
    $startInfo.ArgumentList.Add($argument)
  }

  $process = [Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  try {
    if (-not $process.Start()) {
      throw "$Label native process 沒有啟動"
    }

    # Start both drains before waiting so neither full pipe can deadlock the
    # child. The process wait itself is bounded; a timed-out child is killed as
    # a tree and gets only another bounded interval to disappear.
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    [int] $timeoutMilliseconds = $TimeoutSeconds * 1000
    if (-not $process.WaitForExit($timeoutMilliseconds)) {
      $killFailure = $null
      try {
        $process.Kill($true)
      }
      catch {
        $killFailure = $_.Exception.Message
      }
      $stoppedAfterKill = $process.WaitForExit(5000)
      if (-not $stoppedAfterKill) {
        throw "$Label 超過 $TimeoutSeconds 秒，kill 後 5 秒仍未結束；kill_error=$killFailure"
      }
      throw "$Label 超過 $TimeoutSeconds 秒，已終止；kill_error=$killFailure"
    }

    try {
      $stdout = $stdoutTask.GetAwaiter().GetResult()
      $stderr = $stderrTask.GetAwaiter().GetResult()
    }
    catch {
      throw "$Label stdout/stderr 不是合法 UTF-8：$($_.Exception.Message)"
    }

    return [pscustomobject]@{
      ExitCode = [int] $process.ExitCode
      Stdout = [string] $stdout
      Stderr = [string] $stderr
    }
  }
  finally {
    $process.Dispose()
  }
}

function Assert-NativeUtf8Probe([string] $Python) {
  # Keep the Python program itself ASCII-only: these are fixed UTF-8 bytes, so
  # this proves stdout and stderr decoding instead of merely round-tripping the
  # runner's current code page.
  $probeSource = 'import os; os.write(1, bytes.fromhex("5554462d38207374646f7574efbc9ae4b8ade88fafe99bbbe4bfa10a")); os.write(2, bytes.fromhex("5554462d3820737464657272efbc9ae5b8b3e596aee69fa5e8a9a20a"))'
  $result = Invoke-NativeUtf8 `
    -Path $Python `
    -Arguments @('-c', $probeSource) `
    -TimeoutSeconds 15 `
    -Label 'native UTF-8 雙流 probe'
  $expectedStdout = "UTF-8 stdout：中華電信`n"
  $expectedStderr = "UTF-8 stderr：帳單查詢`n"
  if ($result.ExitCode -ne 0 -or
      $result.Stdout -cne $expectedStdout -or
      $result.Stderr -cne $expectedStderr) {
    throw ("native UTF-8 雙流 probe 不符：exit={0} stdout=<{1}> stderr=<{2}>" -f `
      $result.ExitCode, $result.Stdout, $result.Stderr)
  }
}

function Read-CliVersion([string] $Path, [string] $Label) {
  $result = Invoke-NativeUtf8 `
    -Path $Path `
    -Arguments @('--version') `
    -TimeoutSeconds 15 `
    -Label "$Label --version"
  if ($result.ExitCode -ne 0) {
    throw "$Label --version 失敗，exit=$($result.ExitCode) stdout=$($result.Stdout) stderr=$($result.Stderr)"
  }
  if (-not [string]::IsNullOrEmpty($result.Stderr)) {
    throw "$Label --version 成功卻寫入 stderr：$($result.Stderr)"
  }
  $versionText = $result.Stdout.TrimEnd([char[]] @("`r", "`n"))
  $match = [regex]::Match($versionText, '^sister (?<version>[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+)$')
  if (-not $match.Success) {
    throw "$Label --version 不是唯一一行已知格式：$($result.Stdout)"
  }
  return $match.Groups['version'].Value
}

function Invoke-Setup([string] $Path, [string] $Destination, [string] $Label) {
  $process = Start-Process -FilePath $Path `
    -ArgumentList @('/S', '/NS', "/D=$Destination") -Wait -PassThru
  if ($null -eq $process.ExitCode) {
    throw "$Label setup 沒有可判定的 exit code"
  }
  if ($process.ExitCode -ne 0) {
    throw "$Label setup exit 應為 0，實際 $($process.ExitCode)"
  }
}

function Assert-ProductFiles([string] $InstallDir, [string] $Label) {
  if (-not [IO.Directory]::Exists($InstallDir)) {
    throw "$Label install root 不存在：$InstallDir"
  }
  $actualFiles = @(Get-ChildItem -LiteralPath $InstallDir -File -Force |
    ForEach-Object Name | Sort-Object)
  $actualDirectories = @(Get-ChildItem -LiteralPath $InstallDir -Directory -Recurse -Force)
  $expectedFiles = @('sister-desktop.exe', 'sister.exe', 'uninstall.exe') | Sort-Object
  $difference = @(Compare-Object $expectedFiles $actualFiles)
  if ($difference.Count -ne 0 -or $actualDirectories.Count -ne 0) {
    throw "$Label install root 不是 exact 三檔、零子目錄：files=$($actualFiles -join ', ') dirs=$($actualDirectories.FullName -join ', ')"
  }
}

function Get-RunValue([string] $Name) {
  if (-not (Test-Path -LiteralPath $runKey)) {
    return $null
  }
  $key = Get-Item -LiteralPath $runKey
  try {
    if ($key.GetValueNames() -notcontains $Name) {
      return $null
    }
    return [pscustomobject]@{
      Kind = $key.GetValueKind($Name)
      Value = $key.GetValue(
        $Name,
        $null,
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
  }
  finally {
    $key.Close()
  }
}

function Assert-RunValueAbsent([string] $Name, [string] $Failure) {
  $actual = Get-RunValue $Name
  if ($null -ne $actual) {
    throw "$Failure：kind=$($actual.Kind) value=$($actual.Value)"
  }
}

function Assert-ExactRunString([string] $Name, [string] $Expected, [string] $Failure) {
  $actual = Get-RunValue $Name
  if ($null -eq $actual) {
    throw "$Failure：value 不在"
  }
  if ($actual.Kind -ne [Microsoft.Win32.RegistryValueKind]::String -or
      $actual.Value -cne $Expected) {
    throw "$Failure：kind=$($actual.Kind) value=$($actual.Value) expected=$Expected"
  }
}

function Assert-UninstallMetadata(
  [string] $ExpectedVersion,
  [string] $ExpectedInstallDir,
  [string] $Label
) {
  if (-not (Test-Path -LiteralPath $uninstallKey)) {
    throw "$Label 沒有 current-user uninstall metadata"
  }
  $installed = Get-ItemProperty -LiteralPath $uninstallKey
  if ($null -eq $installed.PSObject.Properties['DisplayVersion'] -or
      $null -eq $installed.PSObject.Properties['InstallLocation']) {
    throw "$Label uninstall metadata 缺 DisplayVersion 或 InstallLocation"
  }
  $quotedInstallDir = '"' + $ExpectedInstallDir + '"'
  if ($installed.DisplayVersion -cne $ExpectedVersion -or
      $installed.InstallLocation -cne $quotedInstallDir) {
    throw "$Label uninstall metadata 不符：version=$($installed.DisplayVersion) root=$($installed.InstallLocation) expected=$ExpectedVersion/$quotedInstallDir"
  }
}

function Get-TreeManifest([string] $Root) {
  if (-not [IO.Directory]::Exists($Root)) {
    throw "外部 state root 不存在，不能把空集合冒充已比對：$Root"
  }
  $files = @(Get-ChildItem -LiteralPath $Root -File -Recurse -Force | Sort-Object FullName)
  if ($files.Count -eq 0) {
    throw "外部 state root 沒有任何檔案，驗不到 preservation：$Root"
  }
  return @($files | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($Root, $_.FullName)
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "${relative}`t$([int64] $_.Length)`t$hash"
  })
}

function Assert-TreeManifest([string] $Root, [string[]] $Expected, [string] $Failure) {
  $actual = @(Get-TreeManifest $Root)
  $difference = @(Compare-Object $Expected $actual)
  if ($difference.Count -ne 0) {
    throw "$Failure：$($difference | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" } | Out-String)"
  }
}

function Assert-BillQuery([string] $Sister, [string] $DataDir, [string] $ConfigPath, [string] $Label) {
  $result = Invoke-NativeUtf8 `
    -Path $Sister `
    -Arguments @('--data-dir', $DataDir, '--config', $ConfigPath, 'query', '電話', '--json') `
    -TimeoutSeconds 30 `
    -Label "$Label query"
  if ($result.ExitCode -ne 0) {
    throw "$Label query 失敗，exit=$($result.ExitCode) stdout=$($result.Stdout) stderr=$($result.Stderr)"
  }
  if (-not [string]::IsNullOrEmpty($result.Stderr)) {
    throw "$Label query 成功卻寫入 stderr：$($result.Stderr)"
  }
  $output = $result.Stdout
  try {
    $json = $output | ConvertFrom-Json
  }
  catch {
    throw "$Label query 沒回合法 JSON：$output"
  }
  if ($json.shape -cne 'keywords') {
    throw "$Label query shape 不是 keywords：$($json.shape)"
  }
  $answers = @($json.answers)
  $hits = @($json.hits)
  if ($answers.Count -ne 2 -or $hits.Count -ne 0) {
    throw "$Label bill fixture 應為 exact 2 answers／0 hits：answers=$($answers.Count) hits=$($hits.Count)"
  }
  $target = @($answers | Where-Object { $_.value -ceq '+886800080123' })
  if ($target.Count -ne 1) {
    throw "$Label query 的 +886800080123 應恰有一筆，實際 $($target.Count)：$output"
  }
  $answer = $target[0]
  if ($answer.raw -cne '0800-080-123' -or
      [int64] $answer.frame_id -ne 1 -or
      [int64] $answer.chunk_id -le 0 -or
      $answer.app_id -cne 'chrome.exe' -or
      $answer.window_title -cne '中華電信 客戶服務 - 帳單查詢' -or
      $answer.url -cne 'https://bill.cht.com.tw/query') {
    throw ("{0} target answer 關聯不符：raw={1} frame={2} chunk={3} app={4} title={5} url={6}" -f `
      $Label,
      $answer.raw,
      $answer.frame_id,
      $answer.chunk_id,
      $answer.app_id,
      $answer.window_title,
      $answer.url)
  }
}

function Get-ConsentSnapshot(
  [string] $Sister,
  [string] $DataDir,
  [string] $ConfigPath,
  [bool] $GrantAll,
  [bool] $ExpectCloudEffective,
  [string] $Label
) {
  $arguments = @('--data-dir', $DataDir, '--config', $ConfigPath, 'consent')
  if ($GrantAll) {
    $arguments += @(
      '--grant', 'local-recording',
      '--grant', 'cloud-reading',
      '--grant', 'frame-storage',
      '--grant', 'azure-tts'
    )
  }
  $arguments += '--json'
  $result = Invoke-NativeUtf8 `
    -Path $Sister `
    -Arguments $arguments `
    -TimeoutSeconds 30 `
    -Label "$Label consent --json"
  if ($result.ExitCode -ne 0) {
    throw "$Label consent --json 失敗，exit=$($result.ExitCode) stdout=$($result.Stdout) stderr=$($result.Stderr)"
  }
  if (-not [string]::IsNullOrEmpty($result.Stderr)) {
    throw "$Label consent --json 成功卻寫入 stderr：$($result.Stderr)"
  }
  $raw = $result.Stdout
  try {
    $json = $raw | ConvertFrom-Json
  }
  catch {
    throw "$Label consent 沒回合法 JSON：$raw"
  }

  $expectedKeys = @('azure-tts', 'cloud-reading', 'frame-storage', 'local-recording')
  $sheets = @($json.sheets | Sort-Object key)
  $actualKeys = @($sheets | ForEach-Object { $_.key })
  if (@(Compare-Object $expectedKeys $actualKeys -CaseSensitive).Count -ne 0) {
    throw "$Label consent sheets 不是 exact 四張：$($actualKeys -join ', ')"
  }
  foreach ($sheet in $sheets) {
    if ($null -eq $sheet.granted_at) {
      throw "$Label consent $($sheet.key) 沒有 granted_at；不能拿 null 冒充已簽"
    }
    [int64] $grantedAt = $sheet.granted_at
    $expectedEffective = if ($sheet.key -ceq 'cloud-reading') {
      $ExpectCloudEffective
    } else {
      $true
    }
    if ($grantedAt -le 0 -or $sheet.effective -ne $expectedEffective) {
      throw "$Label consent $($sheet.key) 狀態不符：granted_at=$grantedAt effective=$($sheet.effective) expected=$expectedEffective"
    }
  }
  foreach ($flag in @('current', 'allows_recording', 'allows_frames', 'allows_azure_tts', 'keeps_images')) {
    if ($json.$flag -ne $true) {
      throw "$Label consent $flag 不是 true：$($json.$flag)"
    }
  }
  if ($json.allows_cloud -ne $ExpectCloudEffective) {
    throw "$Label consent allows_cloud 不符：actual=$($json.allows_cloud) expected=$ExpectCloudEffective"
  }
  if ([int64] $json.version -le 0) {
    throw "$Label consent version 不是已知正值：$($json.version)"
  }
  if ([int64] $json.azure_tts_terms_version -ne 1 -or
      [int64] $json.current_azure_tts_terms_version -ne 1) {
    throw "$Label Azure consent terms 不是 alpha.110 現行 v1：signed=$($json.azure_tts_terms_version) current=$($json.current_azure_tts_terms_version)"
  }

  $grantFingerprint = @($sheets | ForEach-Object {
    "$($_.key):$([int64] $_.granted_at)"
  }) -join '|'
  return [pscustomobject]@{
    Version = [int64] $json.version
    AzureTermsVersion = [int64] $json.azure_tts_terms_version
    CurrentAzureTermsVersion = [int64] $json.current_azure_tts_terms_version
    GrantFingerprint = $grantFingerprint
  }
}

function Assert-SameConsentSnapshot(
  [pscustomobject] $Expected,
  [pscustomobject] $Actual,
  [string] $Failure
) {
  if ($Actual.Version -ne $Expected.Version -or
      $Actual.AzureTermsVersion -ne $Expected.AzureTermsVersion -or
      $Actual.CurrentAzureTermsVersion -ne $Expected.CurrentAzureTermsVersion -or
      $Actual.GrantFingerprint -cne $Expected.GrantFingerprint) {
    throw ("{0}：version/azure/current-azure/grants {1}/{2}/{3}/{4} -> {5}/{6}/{7}/{8}" -f `
      $Failure,
      $Expected.Version,
      $Expected.AzureTermsVersion,
      $Expected.CurrentAzureTermsVersion,
      $Expected.GrantFingerprint,
      $Actual.Version,
      $Actual.AzureTermsVersion,
      $Actual.CurrentAzureTermsVersion,
      $Actual.GrantFingerprint)
  }
}

function Resolve-Python {
  foreach ($name in @('python', 'python3')) {
    $commands = @(Get-Command $name -CommandType Application -ErrorAction SilentlyContinue)
    if ($commands.Count -gt 0) {
      return $commands[0].Source
    }
  }
  throw 'Windows runner 沒有 Python，不能建立可驗證的舊版 DB image_path fixture'
}

function Add-SyntheticEvidence(
  [string] $Python,
  [string] $DbPath,
  [string] $DataDir,
  [string] $FixturePath
) {
  [int64] $fixtureBytes = 10392
  $fixtureSha256 = '989c43933058fc744b5e4af2eef7139e53512946aa536bc99b29d3aca335f788'
  Assert-PublishedFile $FixturePath $fixtureBytes $fixtureSha256 'synthetic evidence source PNG'

  $relativePath = 'synthetic/alpha110-evidence.png'
  $destination = Join-Path (Join-Path $DataDir 'frames') $relativePath
  $destinationParent = Split-Path -Parent $destination
  New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
  Copy-Item -LiteralPath $FixturePath -Destination $destination
  Assert-PublishedFile $destination $fixtureBytes $fixtureSha256 'synthetic evidence copied PNG'

  $env:AI_SISTER_UPGRADE_FIXTURE_DB = $DbPath
  $env:AI_SISTER_UPGRADE_FIXTURE_IMAGE = $relativePath
  try {
    $pythonSource = @'
import os
import sqlite3

db = os.environ["AI_SISTER_UPGRADE_FIXTURE_DB"]
image = os.environ["AI_SISTER_UPGRADE_FIXTURE_IMAGE"]
connection = sqlite3.connect(db)
try:
    frame = connection.execute(
        "SELECT id, image_path, image_bytes FROM frames WHERE id = 1"
    ).fetchone()
    if frame != (1, None, 0):
        raise SystemExit(f"frame 1 不是 replay 的 exact 空圖 fixture: {frame!r}")
    phone = connection.execute(
        "SELECT COUNT(*) FROM text_chunks "
        "WHERE frame_id = 1 AND text LIKE '%0800-080-123%'"
    ).fetchone()[0]
    if phone < 1:
        raise SystemExit("frame 1 沒有客服電話文字，synthetic image 會綁錯來源")
    changed = connection.execute(
        "UPDATE frames SET width = 760, height = 158, image_path = ?, image_bytes = 10392 "
        "WHERE id = 1 AND image_path IS NULL AND image_bytes = 0",
        (image,),
    ).rowcount
    if changed != 1:
        raise SystemExit(f"image_path 應精確更新一列，實際 {changed}")
    foreign_keys = list(connection.execute("PRAGMA foreign_key_check"))
    if foreign_keys:
        raise SystemExit(f"synthetic fixture 破壞 foreign keys: {foreign_keys!r}")
    integrity = list(connection.execute("PRAGMA integrity_check"))
    if integrity != [("ok",)]:
        raise SystemExit(f"synthetic fixture integrity_check 失敗: {integrity!r}")
    connection.commit()
finally:
    connection.close()
'@
    $pythonOutput = @($pythonSource | & $Python - 2>&1)
    $pythonExit = $LASTEXITCODE
  }
  finally {
    Remove-Item Env:AI_SISTER_UPGRADE_FIXTURE_DB -ErrorAction SilentlyContinue
    Remove-Item Env:AI_SISTER_UPGRADE_FIXTURE_IMAGE -ErrorAction SilentlyContinue
  }
  if ($pythonExit -ne 0) {
    throw "建立 synthetic evidence DB fixture 失敗，exit=$pythonExit output=$($pythonOutput -join ' | ')"
  }

  $state = Get-RequiredFileState $destination 'synthetic evidence PNG'
  return [pscustomobject]@{
    RelativePath = $relativePath
    SourcePath = $destination
    State = $state
  }
}

function Assert-SyntheticEvidenceDb(
  [string] $Python,
  [string] $DbPath,
  [string] $Label
) {
  if (-not [IO.File]::Exists($DbPath)) {
    throw "$Label DB 不存在，不能驗 image_path association：$DbPath"
  }
  $env:AI_SISTER_UPGRADE_ASSERT_DB = $DbPath
  try {
    $pythonSource = @'
import os
import sqlite3
from pathlib import Path

db = os.environ["AI_SISTER_UPGRADE_ASSERT_DB"]
connection = sqlite3.connect(Path(db).resolve().as_uri() + "?mode=ro", uri=True)
try:
    frame = connection.execute(
        "SELECT id, width, height, image_path, image_bytes FROM frames WHERE id = 1"
    ).fetchone()
    expected = (1, 760, 158, "synthetic/alpha110-evidence.png", 10392)
    if frame != expected:
        raise SystemExit(f"frame association 不符: {frame!r}; expected={expected!r}")
    phone = connection.execute(
        "SELECT COUNT(*) FROM text_chunks "
        "WHERE frame_id = 1 AND text LIKE '%0800-080-123%'"
    ).fetchone()[0]
    if phone < 1:
        raise SystemExit("frame 1 不再連到客服電話 text chunk")
    foreign_keys = list(connection.execute("PRAGMA foreign_key_check"))
    if foreign_keys:
        raise SystemExit(f"foreign_key_check 失敗: {foreign_keys!r}")
    integrity = list(connection.execute("PRAGMA integrity_check"))
    if integrity != [("ok",)]:
        raise SystemExit(f"integrity_check 失敗: {integrity!r}")
finally:
    connection.close()
'@
    $pythonOutput = @($pythonSource | & $Python - 2>&1)
    $pythonExit = $LASTEXITCODE
  }
  finally {
    Remove-Item Env:AI_SISTER_UPGRADE_ASSERT_DB -ErrorAction SilentlyContinue
  }
  if ($pythonExit -ne 0) {
    throw "$Label synthetic evidence DB 驗證失敗，exit=$pythonExit output=$($pythonOutput -join ' | ')"
  }
}

function Assert-ExportedSyntheticEvidence(
  [string] $Python,
  [string] $Sister,
  [string] $DataDir,
  [string] $ConfigPath,
  [string] $ExportDir,
  [pscustomobject] $Evidence
) {
  if (Test-Path -LiteralPath $ExportDir) {
    throw "export fixture 起點不是空的：$ExportDir"
  }
  $exportResult = Invoke-NativeUtf8 `
    -Path $Sister `
    -Arguments @(
      '--data-dir', $DataDir,
      '--config', $ConfigPath,
      'export', '--to', $ExportDir, '--with-frames'
    ) `
    -TimeoutSeconds 60 `
    -Label 'current export --with-frames'
  if ($exportResult.ExitCode -ne 0) {
    throw ("current export --with-frames 失敗，exit={0} stdout={1} stderr={2}" -f `
      $exportResult.ExitCode, $exportResult.Stdout, $exportResult.Stderr)
  }

  $exportedImage = Join-Path (Join-Path $ExportDir 'frames') $Evidence.RelativePath
  Assert-SameFileState $exportedImage $Evidence.State 'export 沒帶走 exact synthetic evidence PNG'
  $exportedFrameFiles = @(Get-ChildItem -LiteralPath (Join-Path $ExportDir 'frames') `
    -File -Recurse -Force)
  if ($exportedFrameFiles.Count -ne 1) {
    throw "exported frames/ 應只有 exact synthetic PNG，實際：$($exportedFrameFiles.FullName -join ', ')"
  }
  Assert-SyntheticEvidenceDb $Python (Join-Path $ExportDir 'sister.db') 'exported DB'
  $statsResult = Invoke-NativeUtf8 `
    -Path $Sister `
    -Arguments @('--data-dir', $ExportDir, '--config', $ConfigPath, 'stats', '--json') `
    -TimeoutSeconds 30 `
    -Label 'exported DB stats --json'
  if ($statsResult.ExitCode -ne 0) {
    throw ("exported DB stats 失敗，exit={0} stdout={1} stderr={2}" -f `
      $statsResult.ExitCode, $statsResult.Stdout, $statsResult.Stderr)
  }
  if (-not [string]::IsNullOrEmpty($statsResult.Stderr)) {
    throw "exported DB stats 成功卻寫入 stderr：$($statsResult.Stderr)"
  }
  $statsRaw = $statsResult.Stdout
  try {
    $stats = $statsRaw | ConvertFrom-Json
  }
  catch {
    throw "exported DB stats 沒回合法 JSON：$statsRaw"
  }
  if ([int64] $stats.frames_with_image -ne 1) {
    throw "exported DB 的 frames_with_image 應為 exact 1，實際 $($stats.frames_with_image)"
  }
  Assert-BillQuery $Sister $ExportDir $ConfigPath 'exported DB'
}

function Assert-CurrentPayload(
  [string] $InstallDir,
  [string] $CurrentVersion,
  [string] $CurrentSisterPath,
  [string] $CurrentDesktopPath
) {
  Assert-ProductFiles $InstallDir "current $CurrentVersion"
  Assert-UninstallMetadata $CurrentVersion $InstallDir "current $CurrentVersion"
  $installedSister = Join-Path $InstallDir 'sister.exe'
  $installedDesktop = Join-Path $InstallDir 'sister-desktop.exe'
  Assert-ExactFile $installedSister $CurrentSisterPath 'upgrade 後 sister.exe 不是 current exact payload'
  Assert-NsisDesktopPayload $installedDesktop $CurrentDesktopPath
  $reported = Read-CliVersion $installedSister 'installed current sister.exe'
  if ($reported -cne $CurrentVersion) {
    throw "upgrade 後仍不是 current binary：reported=$reported expected=$CurrentVersion"
  }
}

function Invoke-UninstallAndWait([string] $InstallDir, [string] $Label) {
  $uninstaller = Join-Path $InstallDir 'uninstall.exe'
  if (-not [IO.File]::Exists($uninstaller)) {
    throw "$Label 找不到 uninstaller：$uninstaller"
  }
  # NSIS self-copies the uninstaller to temp. Its outer process does not reliably
  # forward the inner exit code, so success is the bounded observed state below,
  # never a convenient wrapper 0.
  $null = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  while (((Test-Path -LiteralPath $InstallDir) -or
          (Test-Path -LiteralPath $uninstallKey) -or
          ($null -ne (Get-RunValue $loginRunName))) -and
         [DateTime]::UtcNow -lt $deadline) {
    Start-Sleep -Milliseconds 250
  }
  if (Test-Path -LiteralPath $InstallDir) {
    throw "$Label uninstaller 沒移除 install root"
  }
  if (Test-Path -LiteralPath $uninstallKey) {
    throw "$Label uninstaller 沒移除 uninstall metadata"
  }
  Assert-RunValueAbsent $loginRunName "$Label uninstaller 沒移除 product Run value"
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue "$Label uninstaller 改動無關 Run value"
}

$currentSetupPath = Resolve-RequiredFile $CurrentSetup 'current setup'
$currentSisterPath = Resolve-RequiredFile $CurrentSister 'current sister.exe'
$currentDesktopPath = Resolve-RequiredFile $CurrentDesktopPortable 'current portable desktop'
$currentTauriConfigPath = Resolve-RequiredFile $CurrentTauriConfig 'current Tauri config'
$scenarioPath = Resolve-RequiredFile $Scenario 'replay scenario'
$evidenceFixturePath = Resolve-RequiredFile $EvidenceFixture 'synthetic evidence source PNG'
$python = Resolve-Python
Assert-NativeUtf8Probe $python

$currentVersion = Read-CliVersion $currentSisterPath 'current sister.exe'
if ($currentVersion -cne $expectedCurrentVersion) {
  throw "這份 upgrade gate 只替 expected current version 作證：reported=$currentVersion expected=$expectedCurrentVersion"
}
try {
  $tauriConfig = Get-Content -LiteralPath $currentTauriConfigPath -Raw | ConvertFrom-Json
}
catch {
  throw "current Tauri config 不是合法 JSON：$($_.Exception.Message)"
}
if ($null -eq $tauriConfig.PSObject.Properties['version'] -or
    [string]::IsNullOrWhiteSpace([string] $tauriConfig.version)) {
  throw 'current Tauri config 沒有已知 version'
}
if ([string] $tauriConfig.version -cne $currentVersion) {
  throw "current CLI 與 installer 版本不一致：CLI=$currentVersion Tauri=$($tauriConfig.version)"
}

$scratchRoot = Join-Path $env:RUNNER_TEMP ("ai-sister-upgrade-{0}" -f [Guid]::NewGuid().ToString('N'))
$baselineSetupPath = Join-Path $scratchRoot 'AI-Sister-alpha.110-Setup.exe'
$absentInstallDir = Join-Path $scratchRoot 'installed-absent'
$enabledInstallDir = Join-Path $scratchRoot 'installed-enabled'
$stateRoot = Join-Path $scratchRoot 'external-state'
$dataDir = Join-Path $stateRoot 'data'
$configPath = Join-Path $stateRoot 'config.toml'
$exportDir = Join-Path $scratchRoot 'exported-memory'

foreach ($path in @(
    $scratchRoot,
    $baselineSetupPath,
    $absentInstallDir,
    $enabledInstallDir,
    $stateRoot,
    $dataDir,
    $configPath,
    $exportDir
  )) {
  if ($path -match '\s') {
    throw "NSIS /D 與這份 smoke 的路徑不可含空白：$path"
  }
}
$trimChars = [char[]] @([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$stateFull = [IO.Path]::GetFullPath($stateRoot).TrimEnd($trimChars)
foreach ($installDir in @($absentInstallDir, $enabledInstallDir)) {
  $installFull = [IO.Path]::GetFullPath($installDir).TrimEnd($trimChars)
  if ($stateFull.StartsWith("$installFull\", [StringComparison]::OrdinalIgnoreCase) -or
      $installFull.StartsWith("$stateFull\", [StringComparison]::OrdinalIgnoreCase)) {
    throw "installer root 與 external state 沒有隔離：install=$installFull state=$stateFull"
  }
}

$registryStartValidated = $false
$createdUnrelatedFixture = $false
$activeInstallDir = $null
try {
  New-Item -ItemType Directory -Path $dataDir | Out-Null
  foreach ($installDir in @($absentInstallDir, $enabledInstallDir)) {
    if (Test-Path -LiteralPath $installDir) {
      throw "upgrade smoke 起點不是 fresh install root：$installDir"
    }
  }
  if (Test-Path -LiteralPath $uninstallKey) {
    throw 'upgrade smoke 起點已有 AI-Sister uninstall metadata，無法證明 old install'
  }
  if (-not (Test-Path -LiteralPath $runKey)) {
    $null = New-Item -Path $runKey -Force
  }
  Assert-RunValueAbsent $loginRunName 'upgrade smoke 起點已有 AI-Sister Run value'
  Assert-RunValueAbsent $unrelatedRunName 'upgrade smoke 的 unrelated Run fixture 名稱已被占用'
  $registryStartValidated = $true
  $null = New-ItemProperty -LiteralPath $runKey -Name $unrelatedRunName `
    -PropertyType String -Value $unrelatedRunValue
  $createdUnrelatedFixture = $true

  Write-Host "下載 immutable baseline：$baselineUrl"
  Invoke-WebRequest -Uri $baselineUrl -OutFile $baselineSetupPath
  Assert-PublishedFile $baselineSetupPath $baselineSetupBytes $baselineSetupSha256 'alpha.110 setup'

  # Lane A: login startup was absent in the old installation. A real cross-version
  # upgrade and a current same-version control must both leave it absent.
  Write-Host "Lane A：安裝舊版 $baselineVersion，Run value 保持 absent"
  $activeInstallDir = $absentInstallDir
  Invoke-Setup $baselineSetupPath $absentInstallDir 'alpha.110 absent lane'
  Assert-ProductFiles $absentInstallDir 'alpha.110 absent lane'
  Assert-UninstallMetadata $baselineVersion $absentInstallDir 'alpha.110 absent lane'
  Assert-RunValueAbsent $loginRunName 'old fresh install 自行打開了登入啟動'
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue 'old install 改動了無關 Run value'

  $absentSister = Join-Path $absentInstallDir 'sister.exe'
  Assert-PublishedFile $absentSister $baselineSisterBytes $baselineSisterSha256 'alpha.110 absent-lane sister.exe'
  $oldVersion = Read-CliVersion $absentSister 'installed old absent-lane sister.exe'
  if ($oldVersion -cne $baselineVersion) {
    throw "下載的 setup 沒有真的安裝舊版：reported=$oldVersion expected=$baselineVersion"
  }

  $configText = @'
[capture]
enabled = true
[privacy]
query_log = false
[shell.persona]
enabled = true
id = "mimo"
motion = false
tap_lines = false
voice_enabled = false
[shell.azure_tts]
enabled = false
region = "japaneast"
voice = "zh-TW-HsiaoYuNeural"
'@
  [IO.File]::WriteAllText($configPath, "$configText`n", $utf8NoBom)
  $sentinelPath = Join-Path $dataDir 'installer-preservation-sentinel.txt'
  [IO.File]::WriteAllText($sentinelPath, "alpha.110 external data`n", $utf8NoBom)

  $oldConsent = Get-ConsentSnapshot $absentSister $dataDir $configPath $true $true 'old installed binary'

  # alpha.110 用 hard link 換名執行時沒有 product event，image-name scan 也看不到；
  # current Setup 仍須靠 installed file 的 image mapping 做跨版本 file-level exclusion。
  $oldRecorderConfigPath = Join-Path $scratchRoot 'old-recorder-config.toml'
  $oldRecorderConfigText = @'
[capture]
enabled = false
'@
  [IO.File]::WriteAllText(
    $oldRecorderConfigPath,
    "$oldRecorderConfigText`n",
    $utf8NoBom
  )
  $oldRecorderLink = Join-Path $scratchRoot 'old-recorder-under-another-name.exe'
  $oldRecorderOut = Join-Path $scratchRoot 'old-recorder-under-another-name.stdout.txt'
  $oldRecorderErr = Join-Path $scratchRoot 'old-recorder-under-another-name.stderr.txt'
  $oldRecorder = $null
  try {
    $null = New-Item -ItemType HardLink -Path $oldRecorderLink -Target $absentSister
    if ((Get-FileHash -LiteralPath $oldRecorderLink -Algorithm SHA256).Hash.ToLowerInvariant() -cne
        $baselineSisterSha256) {
      throw '改名的 alpha.110 recorder hard link 不是 pinned baseline sister.exe'
    }
    $oldRecorder = Start-Process -FilePath $oldRecorderLink `
      -ArgumentList @(
        '--data-dir', $dataDir,
        '--config', $oldRecorderConfigPath,
        'record', '--duration', '300'
      ) `
      -RedirectStandardOutput $oldRecorderOut `
      -RedirectStandardError $oldRecorderErr `
      -PassThru
    $beat = Join-Path $dataDir 'recording.beat'
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath $beat) -and
           [DateTime]::UtcNow -lt $deadline -and -not $oldRecorder.HasExited) {
      Start-Sleep -Milliseconds 250
    }
    if ($oldRecorder.HasExited -or -not (Test-Path -LiteralPath $beat)) {
      $stderr = if (Test-Path -LiteralPath $oldRecorderErr) {
        Get-Content -LiteralPath $oldRecorderErr -Raw
      }
      else {
        '<stderr file absent>'
      }
      throw "改名的 alpha.110 recorder 沒有進入 live heartbeat：$stderr"
    }

    try {
      $eventProbe = [System.Threading.EventWaitHandle]::OpenExisting(
        'Global\com.ted-h.ai-sister-product-lifecycle-v1')
      $eventProbe.Dispose()
      throw '改名的 alpha.110 recorder 意外持有 product lifecycle event'
    }
    catch [System.Threading.WaitHandleCannotBeOpenedException] {
      # Expected: alpha.110 predates the product lifecycle event.
    }
    $productNamed = @(Get-Process -Name 'sister','sister-desktop' `
      -ErrorAction SilentlyContinue)
    if ($productNamed.Count -ne 0) {
      throw "改名的 alpha.110 recorder 不該被 image-name scan 看見：PID=$($productNamed.Id -join ', ')"
    }
    if ($oldRecorder.HasExited) {
      throw '改名的 alpha.110 recorder 在 current Setup 前已退出'
    }

    $refusedSetup = Start-Process -FilePath $currentSetupPath `
      -ArgumentList @('/S', '/NS', "/D=$absentInstallDir") -Wait -PassThru
    try {
      if ($refusedSetup.ExitCode -ne 32) {
        throw "current Setup 面對改名的 alpha.110 recorder 應 exit=32，實際 $($refusedSetup.ExitCode)"
      }
    }
    finally {
      $refusedSetup.Dispose()
    }
    if ($oldRecorder.HasExited) {
      throw 'current Setup file-level exclusion 拒絕後卻殺掉改名的 alpha.110 recorder'
    }
    Assert-ProductFiles $absentInstallDir '跨版本無名 file-level exclusion 拒絕'
    Assert-PublishedFile $absentSister $baselineSisterBytes $baselineSisterSha256 `
      '跨版本無名 file-level exclusion 後的 alpha.110 sister.exe'
    Assert-UninstallMetadata $baselineVersion $absentInstallDir `
      '跨版本無名 file-level exclusion 拒絕'
    $stillOldVersion = Read-CliVersion $absentSister `
      '跨版本無名 file-level exclusion 後的 sister.exe'
    if ($stillOldVersion -cne $baselineVersion) {
      throw "file-level exclusion 拒絕後 sister.exe 版本改變：$stillOldVersion"
    }
    $previous = @(Get-ChildItem -LiteralPath $absentInstallDir -File -Force `
      -Filter '*.ai-sister-previous')
    if ($previous.Count -ne 0) {
      throw "跨版本無名 file-level exclusion 拒絕卻留下改名檔：$($previous.Name -join ', ')"
    }
    try {
      $mutexProbe = [System.Threading.Mutex]::OpenExisting(
        'Global\com.ted-h.ai-sister-install-lifecycle-v1')
      $mutexProbe.Dispose()
      throw '跨版本無名 file-level exclusion 拒絕後 installer lifecycle mutex 仍存在'
    }
    catch [System.Threading.WaitHandleCannotBeOpenedException] {
      # Expected: refused Setup released its lifecycle mutex.
    }

    & $absentSister --data-dir $dataDir stop
    if ($LASTEXITCODE -ne 0) {
      throw '改名的 alpha.110 recorder stop 失敗'
    }
    if (-not $oldRecorder.WaitForExit(30000)) {
      Stop-Process -Id $oldRecorder.Id -Force -ErrorAction SilentlyContinue
      $oldRecorder.WaitForExit()
      throw '改名的 alpha.110 recorder 沒有在 stop 後正常收工'
    }
    if ($oldRecorder.ExitCode -ne 0) {
      throw "改名的 alpha.110 recorder exit=$($oldRecorder.ExitCode)"
    }
    Write-Host '跨版本、無 product 名稱、無 product event 的舊 recorder 仍由 file-level exclusion 拒絕 current Setup'
  }
  finally {
    if ($null -ne $oldRecorder) {
      if (-not $oldRecorder.HasExited) {
        Stop-Process -Id $oldRecorder.Id -Force -ErrorAction SilentlyContinue
        $oldRecorder.WaitForExit()
      }
      $oldRecorder.Dispose()
    }
    if (Test-Path -LiteralPath $oldRecorderLink) {
      try {
        Remove-Item -LiteralPath $oldRecorderLink -Force
      }
      catch {
        # 清 hard link 失敗不該蓋掉上面真正的失敗原因。
        Write-Warning "清掉 $oldRecorderLink 失敗：$($_.Exception.Message)"
      }
    }
  }
  $replayResult = Invoke-NativeUtf8 `
    -Path $absentSister `
    -Arguments @(
      '--data-dir', $dataDir,
      '--config', $configPath,
      'replay', $scenarioPath, '--days-ago', '3'
    ) `
    -TimeoutSeconds 60 `
    -Label 'old installed sister.exe replay DB fixture'
  if (-not [string]::IsNullOrEmpty($replayResult.Stdout)) {
    Write-Host -NoNewline $replayResult.Stdout
  }
  if (-not [string]::IsNullOrEmpty($replayResult.Stderr)) {
    [Console]::Error.Write($replayResult.Stderr)
  }
  if ($replayResult.ExitCode -ne 0) {
    throw ("old installed sister.exe 建立 replay DB fixture 失敗，exit={0} stdout={1} stderr={2}" -f `
      $replayResult.ExitCode, $replayResult.Stdout, $replayResult.Stderr)
  }
  Assert-BillQuery $absentSister $dataDir $configPath 'old installed binary'

  $dbPath = Join-Path $dataDir 'sister.db'
  $consentPath = Join-Path $dataDir 'consent.toml'
  $evidence = Add-SyntheticEvidence $python $dbPath $dataDir $evidenceFixturePath
  Assert-SyntheticEvidenceDb $python $dbPath 'old DB with synthetic evidence'
  Assert-BillQuery $absentSister $dataDir $configPath 'old DB with synthetic evidence'

  $dbBefore = Get-RequiredFileState $dbPath 'old external DB'
  $configBefore = Get-RequiredFileState $configPath 'old external config'
  $consentBefore = Get-RequiredFileState $consentPath 'old external consent'
  $sentinelBefore = Get-RequiredFileState $sentinelPath 'old external data sentinel'
  foreach ($required in @(
      [pscustomobject]@{ Label = 'DB'; State = $dbBefore },
      [pscustomobject]@{ Label = 'config'; State = $configBefore },
      [pscustomobject]@{ Label = 'consent'; State = $consentBefore },
      [pscustomobject]@{ Label = 'data sentinel'; State = $sentinelBefore }
    )) {
    if ($required.State.Length -le 0) {
      throw "$($required.Label) fixture 是空檔，不能拿它驗 preservation"
    }
  }
  $externalBefore = @(Get-TreeManifest $stateRoot)

  Write-Host "Lane A：用 current $currentVersion installer 原地升級 $baselineVersion"
  Invoke-Setup $currentSetupPath $absentInstallDir "current $currentVersion absent lane"

  # These checks happen before the new binary opens the DB. They distinguish
  # "the installer did not touch memory" from a later migration legitimately
  # writing SQLite pages.
  Assert-TreeManifest $stateRoot $externalBefore 'current installer 改動了 install root 外的 state tree'
  Assert-SameFileState $dbPath $dbBefore 'current installer 改動了外部 DB'
  Assert-SameFileState $configPath $configBefore 'current installer 改動了外部 config'
  Assert-SameFileState $consentPath $consentBefore 'current installer 改動了外部 consent'
  Assert-SameFileState $sentinelPath $sentinelBefore 'current installer 改動了外部 data sentinel'
  Assert-CurrentPayload $absentInstallDir $currentVersion $currentSisterPath $currentDesktopPath
  Assert-RunValueAbsent $loginRunName '跨版 upgrade 把 absent 登入啟動自行打開了'
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue '跨版 upgrade 改動了無關 Run value'

  $currentInstalledSister = Join-Path $absentInstallDir 'sister.exe'
  # Db::open runs the migration dispatcher before each command. The baseline is
  # schema 19, so this does not claim a schema step ran when none was needed.
  Assert-BillQuery $currentInstalledSister $dataDir $configPath 'current binary after open/migrate'
  Assert-SyntheticEvidenceDb $python $dbPath 'current DB after open/migrate'
  # Installer 與 migration 要保留四張的原始簽署時間；但 alpha.110 的第二張
  # 只授權「本機先挑候選、CLI 後成句」，不得被 current binary 當成新的
  # 「每題先交 CLI 規劃查詢」授權。所以只有 cloud-reading 必須 fail closed。
  $currentConsent = Get-ConsentSnapshot $currentInstalledSister $dataDir $configPath $false $false 'current binary'
  Assert-SameConsentSnapshot $oldConsent $currentConsent 'current binary 沒保留四張 consent 的 exact timestamps/terms'
  Assert-SameFileState $configPath $configBefore 'current query 改動了外部 config'
  Assert-SameFileState $consentPath $consentBefore 'current query 改動了外部 consent'
  Assert-SameFileState $sentinelPath $sentinelBefore 'current query 改動了外部 data sentinel'
  Assert-ExportedSyntheticEvidence `
    $python $currentInstalledSister $dataDir $configPath $exportDir $evidence

  # This is intentionally named a same-version control. It is the missing absent
  # half of the existing reinstall coverage, never the old->new evidence above.
  $beforeSameVersion = @(Get-TreeManifest $stateRoot)
  Invoke-Setup $currentSetupPath $absentInstallDir "current $currentVersion same-version control"
  Assert-TreeManifest $stateRoot $beforeSameVersion 'current same-version reinstall 改動外部 state'
  Assert-CurrentPayload $absentInstallDir $currentVersion $currentSisterPath $currentDesktopPath
  Assert-RunValueAbsent $loginRunName 'current same-version reinstall 把 absent 登入啟動自行打開了'
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue 'current same-version reinstall 改動無關 Run value'

  $beforeAbsentUninstall = @(Get-TreeManifest $stateRoot)
  Invoke-UninstallAndWait $absentInstallDir 'absent lane'
  $activeInstallDir = $null
  Assert-TreeManifest $stateRoot $beforeAbsentUninstall 'absent-lane uninstall 改動外部 state'

  # Lane B: an exact enabled REG_SZ belongs to the user. The upgrade must retain
  # it byte-for-byte; the real uninstall at the end must remove only this value.
  Write-Host "Lane B：重裝舊版 $baselineVersion，建立 exact enabled Run fixture"
  $activeInstallDir = $enabledInstallDir
  Invoke-Setup $baselineSetupPath $enabledInstallDir 'alpha.110 enabled lane'
  Assert-ProductFiles $enabledInstallDir 'alpha.110 enabled lane'
  Assert-UninstallMetadata $baselineVersion $enabledInstallDir 'alpha.110 enabled lane'
  Assert-RunValueAbsent $loginRunName '第二個 old fresh install 自行打開了登入啟動'
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue '第二個 old install 改動無關 Run value'

  $enabledSister = Join-Path $enabledInstallDir 'sister.exe'
  $enabledDesktop = Join-Path $enabledInstallDir 'sister-desktop.exe'
  Assert-PublishedFile $enabledSister $baselineSisterBytes $baselineSisterSha256 'alpha.110 enabled-lane sister.exe'
  $enabledOldVersion = Read-CliVersion $enabledSister 'installed old enabled-lane sister.exe'
  if ($enabledOldVersion -cne $baselineVersion) {
    throw "第二條 lane 沒有真的裝到舊版：reported=$enabledOldVersion expected=$baselineVersion"
  }
  $expectedLoginRunValue = '"' + $enabledDesktop + '" --ai-sister-login'
  $null = New-ItemProperty -LiteralPath $runKey -Name $loginRunName `
    -PropertyType String -Value $expectedLoginRunValue
  Assert-ExactRunString $loginRunName $expectedLoginRunValue 'old exact enabled Run fixture 讀不回'

  Write-Host "Lane B：用 current $currentVersion installer 原地升級並保留 enabled Run"
  Invoke-Setup $currentSetupPath $enabledInstallDir "current $currentVersion enabled lane"
  Assert-CurrentPayload $enabledInstallDir $currentVersion $currentSisterPath $currentDesktopPath
  Assert-ExactRunString $loginRunName $expectedLoginRunValue '跨版 upgrade 沒保留 exact enabled Run value'
  Assert-ExactRunString $unrelatedRunName $unrelatedRunValue 'enabled-lane upgrade 改動無關 Run value'

  Invoke-UninstallAndWait $enabledInstallDir 'enabled lane'
  $activeInstallDir = $null

  Write-Host "✓ 最後公開 baseline upgrade：$baselineVersion -> $currentVersion；absent/enabled Run 兩面、外部 state、四張 consent、query 與 synthetic evidence export 全部保留"
}
finally {
  # Best-effort cleanup on the disposable runner. Do not erase stateRoot: when a
  # failure occurs, keeping the measured fixtures makes runner diagnostics honest.
  if ($null -ne $activeInstallDir) {
    $uninstaller = Join-Path $activeInstallDir 'uninstall.exe'
    if ([IO.File]::Exists($uninstaller)) {
      try {
        $null = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru
      }
      catch {
        Write-Warning "upgrade smoke cleanup 無法執行 uninstaller：$($_.Exception.Message)"
      }
    }
  }
  if ($registryStartValidated -and (Test-Path -LiteralPath $runKey)) {
    Remove-ItemProperty -LiteralPath $runKey -Name $loginRunName -ErrorAction SilentlyContinue
  }
  if ($createdUnrelatedFixture -and (Test-Path -LiteralPath $runKey)) {
    Remove-ItemProperty -LiteralPath $runKey -Name $unrelatedRunName -ErrorAction SilentlyContinue
  }
}
