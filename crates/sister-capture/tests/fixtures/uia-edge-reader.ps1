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
function Get-SisterFocusedElement {
    try { return [System.Windows.Automation.AutomationElement]::FocusedElement } catch { return $null }
}
function Write-SisterUiaMetadata {
    param([IntPtr]$Hwnd, [string[]]$Extra)
    $node = Get-SisterFocusedElement
    $metadata = @()
    for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
        $current = $node.Current
        $metadata += "$depth type=$($current.ControlType.ProgrammaticName) class=$($current.ClassName) rect=$($current.BoundingRectangle) password=$($current.IsPassword) offscreen=$($current.IsOffscreen) focused=$($current.HasKeyboardFocus) text=$($node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty))"
        if ($current.NativeWindowHandle -eq $Hwnd.ToInt64()) { break }
        $node = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($node)
    }
    if ($Extra) { $metadata += $Extra }
    [IO.File]::WriteAllText((Join-Path $StateDir 'metadata'), ($metadata -join "`n"))
}
function Get-SisterPdfPage {
    try {
        $focused = Get-SisterFocusedElement
        if ($null -eq $focused) { return $null }
        $type = $focused.Current.ControlType
        $doc = $null
        if ($type -eq [System.Windows.Automation.ControlType]::Group) {
            $parent = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($focused)
            if ($null -eq $parent) { return $null }
            if ($parent.Current.ControlType -ne [System.Windows.Automation.ControlType]::Document) { return $null }
            if (-not $parent.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) { return $null }
            $doc = $parent
        } elseif ($type -eq [System.Windows.Automation.ControlType]::Document) {
            if (-not $focused.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) { return $null }
            $doc = $focused
        } else {
            return $null
        }
        $pattern = $doc.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
        $visible = (@($pattern.GetVisibleRanges() | ForEach-Object { $_.GetText(1024) })) -join '|'
        $scope = if ($type -eq [System.Windows.Automation.ControlType]::Group) {
            $pattern.RangeFromChild($focused).GetText(1024)
        } else {
            $visible
        }
        return [pscustomobject]@{
            Kind = if ($type -eq [System.Windows.Automation.ControlType]::Group) { 'Group' } else { 'Document' }
            Offscreen = [bool]$focused.Current.IsOffscreen
            Scope = $scope
            Visible = $visible
            Top = [int]$focused.Current.BoundingRectangle.Y
        }
    } catch {
        return $null
    }
}
function Get-SisterFirstChild($node) {
    if ($null -eq $node) { return $null }
    try { return [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetFirstChild($node) } catch { return $null }
}
function Get-SisterNextSibling($node) {
    if ($null -eq $node) { return $null }
    try { return [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetNextSibling($node) } catch { return $null }
}
function Find-SisterPdfPageGroup {
    param($Start, [string]$Needle, [string]$Exclude, [switch]$AllowOffscreen)
    $script:SisterPdfGroupScan = 'scanned=0 picked=none'
    if ($null -eq $Start) { return $null }
    try {
        $seeds = @()
        $node = $Start
        for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
            $seeds += $node
            $node = Get-SisterParentElement $node
        }
        $queue = New-Object System.Collections.Queue
        foreach ($seed in $seeds) { $queue.Enqueue($seed) }
        $scanned = 0
        $groups = 0
        $large = 0
        $errors = 0
        $max = 128
        while ($queue.Count -gt 0 -and $scanned -lt $max) {
            $current = $queue.Dequeue()
            $scanned++
            try {
                if ($current.Current.ControlType -eq [System.Windows.Automation.ControlType]::Group) {
                    $groups++
                    $rect = $current.Current.BoundingRectangle
                    $offscreen = [bool]$current.Current.IsOffscreen
                    $pageSized = $rect.Width -ge 200 -and $rect.Height -ge 80
                    $scrolledAway = $offscreen -or $rect.Y -lt -100
                    if (($pageSized -and -not $offscreen) -or ($AllowOffscreen -and $scrolledAway)) {
                        $large++
                        $parent = Get-SisterParentElement $current
                        if ($null -ne $parent -and $parent.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document -and $parent.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
                            try {
                                $pattern = $parent.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
                                $text = $pattern.RangeFromChild($current).GetText(1024)
                                if ($text.Contains($Needle) -and -not $text.Contains($Exclude)) {
                                    $script:SisterPdfGroupScan = "scanned=$scanned groups=$groups large=$large picked=group"
                                    return $current
                                }
                            } catch {
                                $errors++
                            }
                        }
                    }
                }
            } catch {}
            $child = Get-SisterFirstChild $current
            while ($null -ne $child) {
                $queue.Enqueue($child)
                $child = Get-SisterNextSibling $child
            }
        }
        $script:SisterPdfGroupScan = "scanned=$scanned groups=$groups large=$large errors=$errors picked=none queued=$($queue.Count)"
    } catch {
        $script:SisterPdfGroupScan = 'error'
    }
    return $null
}
function Invoke-SisterPdfFirstPageGroupFocus {
    try {
        $page = Get-SisterPdfPage
        if ($null -ne $page -and $page.Kind -eq 'Group' -and $page.Scope.Contains('PDF-FIRST') -and -not $page.Scope.Contains('PDF-SECOND') -and -not $page.Offscreen) {
            return
        }
        $group = Find-SisterPdfPageGroup -Start (Get-SisterFocusedElement) -Needle 'PDF-FIRST' -Exclude 'PDF-SECOND'
        if ($null -eq $group) { return }
        $target = $group
        try {
            if (-not $group.Current.IsKeyboardFocusable) {
                $child = Get-SisterFirstChild $group
                $seen = 0
                while ($null -ne $child -and $seen -lt 16) {
                    $seen++
                    if ($child.Current.IsKeyboardFocusable) { $target = $child; break }
                    $deeper = Get-SisterFirstChild $child
                    if ($null -ne $deeper) { $child = $deeper; continue }
                    $child = Get-SisterNextSibling $child
                }
            }
        } catch {}
        try {
            $rect = $target.Current.BoundingRectangle
            $script:SisterPdfGroupScan = "$script:SisterPdfGroupScan focusable=$($target.Current.IsKeyboardFocusable) rect=$([int]$rect.X),$([int]$rect.Y),$([int]$rect.Width),$([int]$rect.Height)"
            if ($rect.Width -gt 1 -and $rect.Height -gt 1) {
                # Top of the page, not the center: PDF-FIRST is near the top,
                # and a center click has left GetVisibleRanges blocked.
                $script:SisterPdfClick = [pscustomobject]@{
                    X = [int]($rect.X + [Math]::Min(40, $rect.Width / 2))
                    Y = [int]($rect.Y + [Math]::Min(48, $rect.Height / 2))
                }
            }
        } catch {}
        $target.SetFocus() | Out-Null
    } catch {}
}
function Get-SisterParentElement($node) {
    if ($null -eq $node) { return $null }
    try { return [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($node) } catch { return $null }
}
function Get-SisterAncestorTextDocument($from) {
    $node = Get-SisterParentElement $from
    for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
        if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document -and $node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
            return $node
        }
        $node = Get-SisterParentElement $node
    }
    return $null
}
function Get-SisterInnerHtmlDocument($from) {
    $node = $from
    for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
        if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document -and -not $node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
            return $node
        }
        $node = Get-SisterParentElement $node
    }
    return $null
}
function Test-SisterHtmlInnerDocumentFocused {
    $focused = Get-SisterFocusedElement
    if ($null -eq $focused) { return $false }
    try {
        if ($focused.Current.ControlType -ne [System.Windows.Automation.ControlType]::Document) { return $false }
        if ($focused.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) { return $false }
        return ($null -ne (Get-SisterAncestorTextDocument $focused))
    } catch {
        return $false
    }
}
function Invoke-SisterHtmlInnerDocumentFocus {
    try {
        $focused = Get-SisterFocusedElement
        if ($null -eq $focused) { return }
        if (Test-SisterHtmlInnerDocumentFocused) { return }
        $inner = Get-SisterInnerHtmlDocument $focused
        $outer = Get-SisterAncestorTextDocument $focused
        if ($null -eq $inner -or $null -eq $outer) { return }
        $inner.SetFocus() | Out-Null
    } catch {}
}
function Get-SisterLivePdfVisible {
    $node = Get-SisterFocusedElement
    $found = @()
    for ($depth = 0; $depth -lt 8 -and $null -ne $node; $depth++) {
        try {
            if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document -and $node.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty)) {
                $pattern = $node.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
                $visible = (@($pattern.GetVisibleRanges() | ForEach-Object { $_.GetText(1024) })) -join '|'
                $found += [pscustomobject]@{ Depth = $depth; Offscreen = [bool]$node.Current.IsOffscreen; Visible = $visible }
            }
        } catch {}
        $node = Get-SisterParentElement $node
    }
    return $found
}
function Invoke-SisterViewportClick {
    param([int]$X, [int]$Y)
    [SisterEdgeWindow]::SetCursorPos($X, $Y) | Out-Null
    [SisterEdgeWindow]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [SisterEdgeWindow]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}
$script:SisterPdfGroupScan = 'unscanned'
$script:SisterPdfClick = $null
$script:SisterPdfMarked = $null
$script:SisterPdfMarkAt = $null
$browser = $null
$backdrop = $null
. (Join-Path $PSScriptRoot 'uia-backdrop.ps1')
try {
    $backdrop = New-SisterUiaBackdrop
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
  if (event.key==='F8') { event.preventDefault(); reader.focus(); reader.scrollTop=reader.scrollHeight; document.title='Sister Edge bottom'; }
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
    $browser = Start-Process -FilePath $edge -ArgumentList @("--user-data-dir=`"$profile`"", '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--hide-crash-restore-bubble', '--force-renderer-accessibility', '--new-window', "`"$uri`"") -PassThru
    [IO.File]::WriteAllText((Join-Path $StateDir 'provider-pid'), [string]$browser.Id)
    $started = [DateTime]::UtcNow
    $deadline = $started.AddSeconds(90)
    $last = ''
    $sent = ''
    $activated = [DateTime]::MinValue
    while ([DateTime]::UtcNow -lt $deadline) {
        [System.Windows.Forms.Application]::DoEvents()
        $browser.Refresh()
        if ($browser.HasExited) { throw 'Owned Edge process exited before the test completed' }
        $request = Join-Path $StateDir 'request'
        $mode = if (Test-Path $request) { [IO.File]::ReadAllText($request) } else { '' }
        if ($mode -eq 'stop') { break }
        $hwnd = $browser.MainWindowHandle
        if ($hwnd -eq [IntPtr]::Zero) {
            if (([DateTime]::UtcNow - $started).TotalSeconds -ge 30) {
                throw "Owned Edge never created a window (title='$($browser.MainWindowTitle)')"
            }
            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'waiting for Edge window')
            Start-Sleep -Milliseconds 40
            continue
        }
        if (-not $mode -or $mode -eq $last) {
            Start-Sleep -Milliseconds 40
            continue
        }
        [SisterEdgeWindow]::SetWindowPos($hwnd, [IntPtr]::new(-1), 40, 40, 760, 620, 0) | Out-Null
        [SisterEdgeWindow]::SetForegroundWindow($hwnd) | Out-Null
        [uint32]$foregroundPid = 0
        $foreground = [SisterEdgeWindow]::GetForegroundWindow()
        [SisterEdgeWindow]::GetWindowThreadProcessId($foreground, [ref]$foregroundPid) | Out-Null
        if ($foreground -ne $hwnd -or $foregroundPid -ne $browser.Id) {
            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'waiting for owned Edge foreground')
            Start-Sleep -Milliseconds 40
            continue
        }
        $stale = ($sent -eq $mode) -and (([DateTime]::UtcNow - $activated).TotalSeconds -ge 1)
        if ($sent -ne $mode) {
            $acted = $false
            if ($Pdf -and $mode -eq 'bottom') {
                # Scroll without transferring accessibility focus. Edge can
                # leave it on the old, now offscreen page; OCR must continue.
                [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'scrolling PDF bottom with old focus')
                [System.Windows.Forms.SendKeys]::SendWait('^{END}{PGDN}{PGDN}')
                $acted = $true
            } elseif ($Pdf -and $mode -ne 'address') {
                if (-not $browser.MainWindowTitle.Contains('reader.pdf')) {
                    [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'waiting for PDF title')
                    Start-Sleep -Milliseconds 100
                    continue
                }
                if ($mode -ne 'top') { throw "Unknown PDF fixture mode: $mode" }
                [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'activating PDF viewport')
                Invoke-SisterViewportClick 400 350
                [System.Windows.Forms.SendKeys]::SendWait('^{HOME}')
                Invoke-SisterPdfFirstPageGroupFocus
                $acted = $true
            } else {
                switch ($mode) {
                    'top' {
                        if ($browser.MainWindowTitle.StartsWith('Sister Edge top')) {
                            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'activating HTML document')
                            Invoke-SisterViewportClick 400 250
                            $acted = $true
                        } else {
                            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'waiting for HTML title')
                        }
                    }
                    'bottom' { [System.Windows.Forms.SendKeys]::SendWait('{F8}'); $acted = $true }
                    'password' { [System.Windows.Forms.SendKeys]::SendWait('{F9}'); $acted = $true }
                    'group' { [System.Windows.Forms.SendKeys]::SendWait('{F10}'); $acted = $true }
                    'address' { [System.Windows.Forms.SendKeys]::SendWait('^l'); $acted = $true }
                    default { throw "Unknown fixture mode: $mode" }
                }
            }
            if (-not $acted) {
                Start-Sleep -Milliseconds 40
                continue
            }
            $sent = $mode
            $activated = [DateTime]::UtcNow
        } elseif ($stale) {
            # Re-send only when the focused control is still the wrong kind or
            # the wrong PDF page. Do not poke a document that is already the
            # provider we want while its text pattern is still filling in.
            if ($Pdf -and $mode -eq 'top') {
                $page = Get-SisterPdfPage
                $documentReady = $null -ne $page -and $page.Kind -eq 'Document' -and -not $page.Offscreen -and (
                    $page.Scope.Contains('PDF-FIRST') -or $page.Visible.Contains('PDF-FIRST')
                )
                $groupReady = $null -ne $page -and $page.Kind -eq 'Group' -and -not $page.Offscreen -and $page.Scope.Contains('PDF-FIRST') -and -not $page.Scope.Contains('PDF-SECOND')
                if (-not $documentReady -and -not $groupReady) {
                    Invoke-SisterPdfFirstPageGroupFocus
                    $activated = [DateTime]::UtcNow
                    [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'PDF focus is not a page group; activating again')
                    $sent = ''
                    continue
                }
            } elseif ($mode -in @('top', 'bottom') -and -not $Pdf) {
                if (-not (Test-SisterHtmlInnerDocumentFocused)) {
                    [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'HTML descendant Group; focusing inner Document')
                    Invoke-SisterHtmlInnerDocumentFocus
                }
            }
        }
        $browser.Refresh()
        $ready = $false
        $extra = @()
        if ($Pdf -and $mode -ne 'address') {
            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'reading direct PDF document')
            Write-SisterUiaMetadata -Hwnd $hwnd -Extra @("group-scan=$script:SisterPdfGroupScan")
            $page = Get-SisterPdfPage
            $ancestors = if ($mode -eq 'bottom') { @(Get-SisterLivePdfVisible) } else { @() }
            $extra += "group-scan=$script:SisterPdfGroupScan"
            if ($null -ne $page) {
                $extra += "focused-page kind=$($page.Kind) scope=[$($page.Scope)] visible=[$($page.Visible)] offscreen=$($page.Offscreen) top=$($page.Top)"
                $extra += (@($ancestors | ForEach-Object { "ancestor$($_.Depth) offscreen=$($_.Offscreen) visible=[$($_.Visible)]" }))
                if ($mode -eq 'bottom') {
                    $live = $ancestors | Where-Object { $_.Visible.Contains('PDF-SECOND') -and -not $_.Visible.Contains('PDF-FIRST') } | Select-Object -First 1
                    if ($null -eq $script:SisterPdfMarkAt -or ((Get-Date) - $script:SisterPdfMarkAt).TotalSeconds -ge 0.5) {
                        $script:SisterPdfMarked = Find-SisterPdfPageGroup -Start (Get-SisterFocusedElement) -Needle 'PDF-FIRST' -Exclude 'PDF-SECOND' -AllowOffscreen
                        $script:SisterPdfMarkAt = Get-Date
                    }
                    $markedOff = $false
                    $markedTop = 0
                    if ($null -ne $script:SisterPdfMarked) {
                        $markedOff = [bool]$script:SisterPdfMarked.Current.IsOffscreen
                        $markedTop = [int]$script:SisterPdfMarked.Current.BoundingRectangle.Y
                    }
                    $extra += "marked-page offscreen=$markedOff top=$markedTop"
                    $groupOffscreen = $page.Kind -eq 'Group' -and $page.Offscreen -and $page.Scope.Contains('PDF-FIRST') -and (
                        ($null -ne $live) -or ($page.Top -lt -100)
                    )
                    # Focus may stay on the Document. Ctrl+End moves the
                    # first-page Group above the viewport while visible ranges
                    # still omit PDF-SECOND.
                    $pageMoved = $null -ne $script:SisterPdfMarked -and ($markedOff -or $markedTop -lt -100)
                    $documentScrolled = $page.Kind -eq 'Document' -and -not $page.Offscreen -and $page.Visible.Contains('PDF-SECOND')
                    $ready = $groupOffscreen -or $documentScrolled -or $pageMoved
                } else {
                    $groupTop = $page.Kind -eq 'Group' -and -not $page.Offscreen -and $page.Scope.Contains('PDF-FIRST') -and -not $page.Scope.Contains('PDF-SECOND') -and $page.Visible.Contains('PDF-FIRST')
                    $documentTop = $page.Kind -eq 'Document' -and -not $page.Offscreen -and (
                        $page.Scope.Contains('PDF-FIRST') -or $page.Visible.Contains('PDF-FIRST')
                    )
                    $ready = $groupTop -or $documentTop
                }
            } else {
                $extra += 'focused-page=none'
            }
        } elseif ($mode -in @('top', 'bottom')) {
            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), 'waiting for HTML inner Document')
            if (-not (Test-SisterHtmlInnerDocumentFocused)) {
                Invoke-SisterHtmlInnerDocumentFocus
            }
            $ready = (Test-SisterHtmlInnerDocumentFocused) -and $browser.MainWindowTitle.StartsWith("Sister Edge $mode")
        } elseif ($Pdf -or $mode -eq 'address' -or $browser.MainWindowTitle.StartsWith("Sister Edge $mode")) {
            $ready = $true
        }
        Write-SisterUiaMetadata -Hwnd $hwnd -Extra $extra
        if ($ready) {
            [IO.File]::WriteAllText((Join-Path $StateDir 'stage'), "ready $mode")
            [IO.File]::WriteAllText((Join-Path $StateDir 'ready'), $mode)
            $last = $mode
        }
        Start-Sleep -Milliseconds 40
    }
} catch {
    [IO.File]::WriteAllText((Join-Path $StateDir 'error'), $_.Exception.ToString())
} finally {
    if ($backdrop) { $backdrop.Dispose() }
    if ($browser) {
        # Unique --user-data-dir means this PID/tree never belongs to a user's Edge session.
        & taskkill.exe /PID $browser.Id /T /F 2>&1 | Out-Null
    }
}
