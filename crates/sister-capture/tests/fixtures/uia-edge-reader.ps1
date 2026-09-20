# Owned Edge profile + local HTML/PDF. Only the fixture process tree is controlled.
param([Parameter(Mandatory=$true)][string]$StateDir, [switch]$Pdf)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SisterEdgeWindow {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
}
'@
$browser = $null
try {
    $edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $edge) { throw 'Microsoft Edge is not installed on the native runner' }
    $html = @'
<!doctype html><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'">
<title>Sister Edge loading</title>
<style>body{font:20px sans-serif;margin:24px}main{height:280px;overflow:auto;border:1px solid;padding:8px}p{margin:16px 0}</style>
<main id="reader" role="document" tabindex="0" autofocus>
<p>&#x7db2;&#x9801;&#x96fb;&#x8a71; 0800-333-444</p><p>EDGE-SECOND-PARAGRAPH</p>
<div id="unsupported" role="group" tabindex="0">GROUP-PARENT-SENTINEL</div>
<div style="height:4000px">padding</div><p>EDGE-BOTTOM 02-7766-5544</p>
<p hidden>HIDDEN-SENTINEL</p></main>
<p>SIBLING-SENTINEL 0800-999-000</p>
<label>Password <input id="secret" type="password" value="PASSWORD-SENTINEL"></label>
<script>
const reader = document.getElementById('reader');
window.addEventListener('load', () => { reader.focus(); document.title='Sister Edge top'; });
window.addEventListener('keydown', event => {
  if (event.key==='F8') { event.preventDefault(); reader.scrollTop=reader.scrollHeight; document.title='Sister Edge bottom'; }
  if (event.key==='F9') { event.preventDefault(); document.getElementById('secret').focus(); document.title='Sister Edge password'; }
  if (event.key==='F10') { event.preventDefault(); document.getElementById('unsupported').focus(); document.title='Sister Edge group'; }
});
</script>
'@
    if ($Pdf) {
        # Original two-page, text-only PDF. Exact ASCII stream lengths and xref
        # offsets; no downloaded sample, embedded scripts, forms or external links.
        $streams = @(
            "BT /F1 20 Tf 60 680 Td (PDF-FIRST phone 0800-444-555) Tj ET`n",
            "BT /F1 20 Tf 60 120 Td (PDF-SECOND phone 02-6655-4433) Tj ET`n"
        )
        $objects = @(
            '<< /Type /Catalog /Pages 2 0 R >>',
            '<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>',
            '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>',
            '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>',
            '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
            "<< /Length $($streams[0].Length) >>`nstream`n$($streams[0])endstream",
            "<< /Length $($streams[1].Length) >>`nstream`n$($streams[1])endstream"
        )
        $pdfBody = "%PDF-1.4`n"
        $offsets = @()
        for ($index = 0; $index -lt $objects.Count; $index++) {
            $offsets += $pdfBody.Length
            $pdfBody += "$($index + 1) 0 obj`n$($objects[$index])`nendobj`n"
        }
        $xref = $pdfBody.Length
        $pdfBody += "xref`n0 8`n0000000000 65535 f `n"
        foreach ($offset in $offsets) { $pdfBody += ('{0:D10} 00000 n ' -f $offset) + "`n" }
        $pdfBody += "trailer`n<< /Size 8 /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n"
        $documentPath = Join-Path $StateDir 'reader.pdf'
        [IO.File]::WriteAllText($documentPath, $pdfBody, [Text.Encoding]::ASCII)
    } else {
        $documentPath = Join-Path $StateDir 'reader.html'
        [IO.File]::WriteAllText($documentPath, $html, [Text.UTF8Encoding]::new($false))
    }
    [IO.File]::WriteAllText((Join-Path $StateDir 'document-name'), [IO.Path]::GetFileName($documentPath))
    $uri = ([Uri]$documentPath).AbsoluteUri
    $profile = Join-Path $StateDir 'edge-profile'
    $browser = Start-Process -FilePath $edge -ArgumentList @("--user-data-dir=`"$profile`"", '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--force-renderer-accessibility', '--new-window', "`"$uri`"") -PassThru
    [IO.File]::WriteAllText((Join-Path $StateDir 'provider-pid'), [string]$browser.Id)
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $last = ''
    $sent = ''
    $activated = [DateTime]::MinValue
    while ([DateTime]::UtcNow -lt $deadline) {
        $browser.Refresh()
        if ($browser.HasExited) { throw 'Owned Edge process exited before the test completed' }
        $request = Join-Path $StateDir 'request'
        $mode = if (Test-Path $request) { [IO.File]::ReadAllText($request) } else { '' }
        if ($mode -eq 'stop') { break }
        $hwnd = $browser.MainWindowHandle
        if ($mode -and $mode -ne $last -and $hwnd -ne [IntPtr]::Zero) {
            [SisterEdgeWindow]::SetWindowPos($hwnd, [IntPtr]::new(-1), 40, 40, 760, 620, 0) | Out-Null
            [SisterEdgeWindow]::SetForegroundWindow($hwnd) | Out-Null
            [uint32]$foregroundPid = 0
            $foreground = [SisterEdgeWindow]::GetForegroundWindow()
            [SisterEdgeWindow]::GetWindowThreadProcessId($foreground, [ref]$foregroundPid) | Out-Null
            if ($foreground -ne $hwnd -or $foregroundPid -ne $browser.Id) { Start-Sleep -Milliseconds 40; continue }
            if ($sent -ne $mode) {
                if ($Pdf -and $mode -ne 'address') {
                    if (-not $browser.MainWindowTitle.Contains('reader.pdf') -or ([DateTime]::UtcNow - $activated).TotalSeconds -lt 1) {
                        Start-Sleep -Milliseconds 100
                        continue
                    }
                    # Only activate our verified foreground viewport. Separate
                    # clicks cannot be interpreted as PDF double-click zoom.
                    [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), "paging PDF $mode")
                    [SisterEdgeWindow]::SetCursorPos(400, 350) | Out-Null
                    [SisterEdgeWindow]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
                    [SisterEdgeWindow]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
                    $activated = [DateTime]::UtcNow
                    if ($mode -ne 'top') { throw "Unknown PDF fixture mode: $mode" }
                    [System.Windows.Forms.SendKeys]::SendWait('^{HOME}')
                } else {
                    switch ($mode) {
                        'top' { }
                        'bottom' { [System.Windows.Forms.SendKeys]::SendWait('{F8}') }
                        'password' { [System.Windows.Forms.SendKeys]::SendWait('{F9}') }
                        'group' { [System.Windows.Forms.SendKeys]::SendWait('{F10}') }
                        'address' { [System.Windows.Forms.SendKeys]::SendWait('^l') }
                        default { throw "Unknown fixture mode: $mode" }
                    }
                }
                $sent = $mode
            }
            $browser.Refresh()
            if ($Pdf -or $mode -eq 'address' -or $browser.MainWindowTitle.StartsWith("Sister Edge $mode")) {
                # Only describe metadata under this owned foreground window. This
                # makes a native provider mismatch diagnosable without product logging.
                $node = [System.Windows.Automation.AutomationElement]::FocusedElement
                $metadata = @()
                for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
                    $current = $node.Current
                    $metadata += "$depth type=$($current.ControlType.ProgrammaticName) class=$($current.ClassName) rect=$($current.BoundingRectangle) password=$($current.IsPassword) offscreen=$($current.IsOffscreen) focused=$($current.HasKeyboardFocus) text=$($node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty))"
                    if ($Pdf -and $node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
                        $pattern = $node.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
                        $visible = @($pattern.GetVisibleRanges() | ForEach-Object { $_.GetText(1024) })
                        $metadata += "  visible=[$($visible -join '|')]"
                    }
                    if ($current.NativeWindowHandle -eq $hwnd.ToInt64()) { break }
                    $node = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($node)
                }
                if ($Pdf -and $mode -ne 'address') {
                    # Wait for the native PDF accessibility provider to expose
                    # its document. Do not accept or manufacture captured text.
                    $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
                    $expectedPage = 'PDF-FIRST'
                    $pageReady = $false
                    if ($focused.Current.ControlType -eq [System.Windows.Automation.ControlType]::Group) {
                        $parent = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($focused)
                        if ($parent.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document -and $parent.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
                            $pattern = $parent.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
                            $scope = $pattern.RangeFromChild($focused).GetText(1024)
                            $visible = (@($pattern.GetVisibleRanges() | ForEach-Object { $_.GetText(1024) })) -join '|'
                            $pageReady = -not $focused.Current.IsOffscreen -and $scope.Contains($expectedPage) -and $visible.Contains($expectedPage)
                            $metadata += "focused-page=[$scope] visible=[$visible]"

                        }
                    }
                    [IO.File]::WriteAllText((Join-Path $StateDir 'metadata'), ($metadata -join "`n"))
                    if (-not $pageReady) {
                        $sent = ''
                        Start-Sleep -Milliseconds 100
                        continue
                    }
                }
                [IO.File]::WriteAllText((Join-Path $StateDir 'metadata'), ($metadata -join "`n"))
                [IO.File]::WriteAllText((Join-Path $StateDir 'ready'), $mode)
                $last = $mode
            }
        }
        Start-Sleep -Milliseconds 40
    }
} catch {
    [IO.File]::WriteAllText((Join-Path $StateDir 'error'), $_.Exception.ToString())
} finally {
    if ($browser) {
        # Unique --user-data-dir means this PID/tree never belongs to a user's Edge session.
        & taskkill.exe /PID $browser.Id /T /F 2>&1 | Out-Null
    }
}
