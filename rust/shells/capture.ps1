param(
    [Parameter(Mandatory)]
    [ValidateSet('desktop', 'web', 'android')]
    [string]$Target,
    # Directory the PNGs are written to.
    [Parameter(Mandatory)]
    [string]$Out,
    # Prefix of every file name, e.g. `before` or `after`.
    [string]$Label = 'capture',
    [string]$TargetDir = $(if ($env:T3_SHELLS_TARGET) { $env:T3_SHELLS_TARGET } else { '' }),
    [string]$Device,
    [int]$Port = 18764,
    # A Chromium-family browser for the web captures; Edge or Chrome is found if omitted.
    [string]$Browser
)

# Drives the slice the same way on every target: pair with the demo server, capture the thread
# list, open the first thread, capture it. Starts only what it captures and stops it by the PID
# or package it started.

$ErrorActionPreference = 'Stop'
$shellRoot = $PSScriptRoot
$buildRoot = if ($TargetDir) { [IO.Path]::GetFullPath($TargetDir) } else { Join-Path $shellRoot 'target' }
$outRoot = Join-Path $buildRoot 'out'
$appName = 'T3 Code Dev'
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

function Wait-Until([scriptblock]$Condition, [int]$Seconds, [string]$What) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $value = & $Condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 250
    }
    throw "Timed out waiting for $What"
}

function Invoke-DesktopCapture {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
    Add-Type -Namespace T3Capture -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
public struct RECT { public int Left, Top, Right, Bottom; }
'@
    $exe = Join-Path $buildRoot 'debug/t3-desktop.exe'
    if (-not (Test-Path $exe)) { throw "Build the desktop app first: $exe" }
    $process = Start-Process -FilePath $exe -PassThru
    try {
        $handle = Wait-Until { $process.Refresh(); if ($process.MainWindowHandle -ne 0) { $process.MainWindowHandle } } 60 'the desktop window'
        $root = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
        $find = {
            param($name)
            $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $name)
            $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
        }
        $invoke = {
            param($name)
            $element = Wait-Until { & $find $name } 30 "'$name'"
            $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        }
        $shot = {
            param($name)
            $rect = New-Object T3Capture.Native+RECT
            [T3Capture.Native]::GetWindowRect($handle, [ref]$rect) | Out-Null
            $bitmap = [System.Drawing.Bitmap]::new($rect.Right - $rect.Left, $rect.Bottom - $rect.Top)
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            $hdc = $graphics.GetHdc()
            [T3Capture.Native]::PrintWindow($handle, $hdc, 2) | Out-Null
            $graphics.ReleaseHdc($hdc)
            $path = Join-Path $Out "$Label-desktop-$name.png"
            $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
            $graphics.Dispose(); $bitmap.Dispose()
            Write-Output $path
        }
        & $invoke 'Fill in the demo server'
        & $invoke 'Connect'
        Wait-Until { & $find 'Build the shared Rust UI slice' } 30 'the thread list' | Out-Null
        Start-Sleep -Milliseconds 700
        & $shot 'home'
        & $invoke 'Build the shared Rust UI slice'
        Start-Sleep -Milliseconds 1500
        & $shot 'thread'
    } finally {
        if (-not $process.HasExited) { $process.Kill() }
    }
}

function Invoke-WebCapture {
    $site = Join-Path $outRoot $appName
    if (-not (Test-Path (Join-Path $site 'index.html'))) { throw "Build the web site first: $site" }
    if (-not $Browser) {
        $Browser = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Google\Chrome\Application\chrome.exe") |
            Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $Browser) { throw 'No Chromium browser found; pass -Browser' }
    # Start-Process joins arguments with spaces, so the site path (which has spaces) is quoted.
    $server = Start-Process -FilePath python -ArgumentList "-m http.server $Port --bind 127.0.0.1 --directory `"$site`"" -PassThru -WindowStyle Hidden
    try {
        Wait-Until { try { (Invoke-WebRequest "http://127.0.0.1:$Port/" -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200 } catch { $false } } 30 'the static server' | Out-Null
        & node (Join-Path $shellRoot 'capture-web.mjs') "http://127.0.0.1:$Port/" $Out $Label $Browser
        if ($LASTEXITCODE -ne 0) { throw "web capture failed ($LASTEXITCODE)" }
    } finally {
        if (-not $server.HasExited) { $server.Kill() }
    }
}

function Invoke-AndroidCapture {
    $sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android/Sdk' }
    $adb = Join-Path $sdk 'platform-tools/adb.exe'
    $apk = Join-Path $outRoot "$appName.apk"
    if (-not (Test-Path $apk)) { throw "Build the APK first: $apk" }
    $device = if ($Device) { @('-s', $Device) } else { @() }
    $package = 'codes.t3.dev'
    & $adb @device install -r $apk | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'adb install failed' }
    & $adb @device shell am force-stop $package
    & $adb @device shell am start -W -n "$package/com.longbridge.nsh.AppActivity" | Out-Null
    $locate = {
        param($label)
        Wait-Until {
            & $adb @device shell uiautomator dump /sdcard/t3-ui.xml | Out-Null
            $xml = [xml](& $adb @device shell cat /sdcard/t3-ui.xml)
            $node = $xml.SelectNodes('//node') | Where-Object { $_.text -eq $label -or $_.'content-desc' -eq $label } | Select-Object -First 1
            if ($node) { $node.bounds }
        } 60 "'$label' on the device"
    }
    $tap = {
        param($label)
        $bounds = & $locate $label
        if ($bounds -notmatch '\[(\d+),(\d+)\]\[(\d+),(\d+)\]') { throw "bad bounds $bounds" }
        $x = ([int]$Matches[1] + [int]$Matches[3]) / 2; $y = ([int]$Matches[2] + [int]$Matches[4]) / 2
        & $adb @device shell input tap ([int]$x) ([int]$y)
    }
    $shot = {
        param($name)
        $path = Join-Path $Out "$Label-android-$name.png"
        & $adb @device shell screencap -p /sdcard/t3-ui.png
        & $adb @device pull /sdcard/t3-ui.png $path | Out-Null
        Write-Output $path
    }
    try {
        & $tap 'Fill in the demo server'
        & $tap 'Connect'
        & $locate 'Build the shared Rust UI slice' | Out-Null
        Start-Sleep -Milliseconds 700
        & $shot 'home'
        & $tap 'Build the shared Rust UI slice'
        Start-Sleep -Milliseconds 1500
        & $shot 'thread'
    } finally {
        & $adb @device shell rm -f /sdcard/t3-ui.xml /sdcard/t3-ui.png
        & $adb @device shell am force-stop $package
    }
}

switch ($Target) {
    'desktop' { Invoke-DesktopCapture }
    'web' { Invoke-WebCapture }
    'android' { Invoke-AndroidCapture }
}
