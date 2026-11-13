param(
    [Parameter(Mandatory)]
    [ValidateSet('install-cli', 'desktop', 'desktop-build', 'gpui', 'web', 'serve', 'android', 'install-android', 'ios', 'ios-simulator', 'macos', 'check', 'all')]
    [string]$Action,
    [string]$Abi = 'x86_64',
    [string]$Device,
    [int]$Port = 18764,
    [string]$SigningIdentity,
    [string]$ProvisioningProfile,
    # Build output (Cargo targets, the CLI, runtimes) and packages (web site, APK). Defaults to
    # $env:T3_SHELLS_TARGET, else rust/shells/target; keep it off the worktree's drive if space is short.
    [string]$TargetDir = $(if ($env:T3_SHELLS_TARGET) { $env:T3_SHELLS_TARGET } else { '' }),
    [switch]$Run
)

$ErrorActionPreference = 'Stop'
$shellRoot = $PSScriptRoot
$repoRoot = [IO.Path]::GetFullPath((Join-Path $shellRoot '../..'))
$uiManifest = Join-Path $repoRoot 'rust/crates/t3-ui/Cargo.toml'
$buildRoot = if ($TargetDir) { [IO.Path]::GetFullPath($TargetDir) } else { Join-Path $shellRoot 'target' }
$outRoot = Join-Path $buildRoot 'out'
$appName = 'T3 Code Dev'
$env:CARGO_TARGET_DIR = $buildRoot
$env:RUST_MIN_STACK = '268435456'

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program exited with $LASTEXITCODE" }
}

function Get-NshPin {
    $metadata = & cargo metadata --manifest-path $uiManifest --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read UI manifest' }
    $package = ($metadata | ConvertFrom-Json).packages | Where-Object name -EQ 't3-ui'
    $sources = @($package.dependencies | Where-Object { $_.name -like 'nsh-*' } | ForEach-Object source | Sort-Object -Unique)
    if ($sources.Count -ne 1 -or $sources[0] -notmatch '^git\+(.+)\?rev=([a-f0-9]{40})$') {
        throw 'All NSH dependencies must use the same exact Git revision in t3-ui/Cargo.toml'
    }
    return @{ Url = $Matches[1]; Revision = $Matches[2] }
}

function Get-NshSource {
    $metadata = & cargo metadata --manifest-path $uiManifest --locked --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the pinned NSH source' }
    $core = ($metadata | ConvertFrom-Json).packages | Where-Object name -EQ 'nsh-core'
    return [IO.Path]::GetFullPath((Join-Path (Split-Path $core.manifest_path -Parent) '../..'))
}

function Initialize-DesktopManifests($Pin) {
    foreach ($shell in @('desktop', 'gpui')) {
        $template = Get-Content (Join-Path $shellRoot "$shell/Cargo.toml.in") -Raw
        $manifest = $template.Replace('@NSH_GIT@', $Pin.Url).Replace('@NSH_REV@', $Pin.Revision)
        Set-Content (Join-Path $shellRoot "$shell/Cargo.toml") $manifest -NoNewline -Encoding utf8NoBOM
    }
}

# The desktop runners read `asset:/@<package>/<path>` from `assets` beside the executable (the
# layout `nsh package` produces); a cargo-built shell gets the UI crate's files copied there.
function Copy-DesktopAssets([string]$BuildProfile) {
    $destination = Join-Path $buildRoot "$BuildProfile/assets/@t3-ui"
    if (Test-Path $destination) { Remove-Item -Recurse -Force $destination }
    New-Item -ItemType Directory -Force $destination | Out-Null
    Copy-Item -Recurse (Join-Path (Split-Path $uiManifest -Parent) 'assets/*') $destination
}

function Initialize-Wasm {
    Invoke-Checked rustup @('target', 'add', 'wasm32-unknown-unknown')
    $env:CC_wasm32_unknown_unknown = (Get-Command clang -ErrorAction Stop).Source
    $env:AR_wasm32_unknown_unknown = (Get-Command llvm-ar -ErrorAction Stop).Source
}

function Initialize-Android {
    $sdk = $env:ANDROID_HOME
    if (-not $sdk) { $sdk = $env:ANDROID_SDK_ROOT }
    if (-not $sdk -and $IsWindows) { $sdk = Join-Path $env:LOCALAPPDATA 'Android/Sdk' }
    if (-not $sdk -or -not (Test-Path $sdk)) { throw 'Set ANDROID_HOME to an installed Android SDK' }
    $env:ANDROID_HOME = $sdk
    $env:ANDROID_SDK_ROOT = $sdk
    $jdk = $env:JAVA_HOME
    if (-not $jdk) {
        $settings = & javac -J-XshowSettings:properties -version 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or $settings -notmatch '(?m)^\s*java\.home = ([^\r\n]+)') {
            throw 'Cannot discover the JDK; set JAVA_HOME to JDK 17 or newer'
        }
        $jdk = $Matches[1].Trim()
    }
    $env:JAVA_HOME = $jdk
    $env:PATH = "$(Join-Path $jdk 'bin')$([IO.Path]::PathSeparator)$env:PATH"
    $buildTools = Get-ChildItem (Join-Path $sdk 'build-tools') -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    $androidJar = Join-Path $sdk 'platforms/android-35/android.jar'
    if (-not (Test-Path $androidJar)) { throw 'Install Android SDK platform 35' }
    if (-not $env:ANDROID_NDK_HOME) {
        $ndk = Get-ChildItem (Join-Path $sdk 'ndk') -Directory | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
        $env:ANDROID_NDK_HOME = $ndk.FullName
    }
    $runtimeRoot = Join-Path $buildRoot 'runtime'
    $runtime = Join-Path $runtimeRoot 'android/release'
    New-Item -ItemType Directory -Force (Join-Path $runtime 'classes'), (Join-Path $runtime "lib/$Abi") | Out-Null
    $source = Get-NshSource
    $javaSources = Get-ChildItem (Join-Path $source 'crates/nsh-platform-android/java') -Recurse -Filter '*.java' |
        ForEach-Object { '"' + $_.FullName.Replace('\', '/') + '"' }
    $argsFile = Join-Path $runtime 'sources.txt'
    Set-Content $argsFile $javaSources -Encoding utf8NoBOM
    Invoke-Checked (Join-Path $jdk 'bin/javac') @('--release', '17', '-encoding', 'UTF-8', '-classpath', $androidJar, '-d', (Join-Path $runtime 'classes'), "@$argsFile")
    Invoke-Checked (Join-Path $jdk 'bin/jar') @('cf', (Join-Path $runtime 'classes.jar'), '-C', (Join-Path $runtime 'classes'), '.')
    $d8 = if ($IsWindows) { 'd8.bat' } else { 'd8' }
    Invoke-Checked (Join-Path $buildTools.FullName $d8) @('--min-api', '26', '--lib', $androidJar, '--output', $runtime, (Join-Path $runtime 'classes.jar'))
    $env:NSH_RUNTIME = $runtimeRoot
    $triple = switch ($Abi) {
        'x86_64' { 'x86_64-linux-android' }
        'arm64-v8a' { 'aarch64-linux-android' }
        'armeabi-v7a' { 'armv7-linux-androideabi' }
        'x86' { 'i686-linux-android' }
        default { throw "Unsupported ABI: $Abi" }
    }
    Invoke-Checked rustup @('target', 'add', $triple)
}

function Invoke-Package([string]$Target, [string]$Cli, $Pin) {
    $cliVersion = & $Cli --version
    if ($LASTEXITCODE -ne 0 -or $cliVersion -notlike "*$($Pin.Revision.Substring(0, 7))*") {
        throw 'Installed NSH CLI does not match the UI pin; run ui.ps1 install-cli'
    }
    if ($Target -eq 'web') { Initialize-Wasm }
    if ($Target -eq 'android') { Initialize-Android }
    $generatedWorkspace = Join-Path $shellRoot '.nsh/rust'
    $generatedLock = Join-Path $generatedWorkspace 'Cargo.lock'
    if (-not (Test-Path $generatedLock)) {
        New-Item -ItemType Directory -Force $generatedWorkspace | Out-Null
        Copy-Item (Join-Path $shellRoot 'packaging.Cargo.lock') $generatedLock
    }
    $packageArgs = @('package', '--target', $Target, '-v', '-o', $outRoot)
    if ($Target -eq 'android') { $packageArgs += @('--abi', $Abi) }
    if ($SigningIdentity) { $packageArgs += @('--sign', $SigningIdentity) }
    if ($ProvisioningProfile) { $packageArgs += @('--profile', $ProvisioningProfile) }
    if ($Run) { $packageArgs += '--run' }
    Invoke-Checked $Cli $packageArgs
}

Push-Location $shellRoot
try {
    $pin = Get-NshPin
    Initialize-DesktopManifests $pin
    $cli = Join-Path $buildRoot $(if ($IsWindows) { 'tools/bin/nsh.exe' } else { 'tools/bin/nsh' })
    switch ($Action) {
        'install-cli' {
            $env:CARGO_TARGET_DIR = Join-Path $buildRoot 'cli-build'
            Invoke-Checked cargo @('install', '--git', $pin.Url, '--rev', $pin.Revision, '--locked', '--debug', '--root', (Join-Path $buildRoot 'tools'), 'nsh-cli')
        }
        'desktop' {
            Copy-DesktopAssets 'debug'
            Invoke-Checked cargo @('run', '--locked', '--manifest-path', (Join-Path $shellRoot 'desktop/Cargo.toml'))
        }
        'desktop-build' {
            Invoke-Checked cargo @('build', '--locked', '--manifest-path', (Join-Path $shellRoot 'desktop/Cargo.toml'))
            Copy-DesktopAssets 'debug'
            Write-Output "desktop: $(Join-Path $buildRoot 'debug/t3-desktop.exe')"
        }
        'gpui' {
            Copy-DesktopAssets 'debug'
            Invoke-Checked cargo @('run', '--locked', '--manifest-path', (Join-Path $shellRoot 'gpui/Cargo.toml'))
        }
        'serve' { Invoke-Checked python @('-m', 'http.server', "$Port", '--bind', '127.0.0.1', '--directory', (Join-Path $outRoot $appName)) }
        'install-android' {
            $adbArgs = if ($Device) { @('-s', $Device) } else { @() }
            Invoke-Checked adb ($adbArgs + @('install', '-r', (Join-Path $outRoot "$appName.apk")))
            Invoke-Checked adb ($adbArgs + @('shell', 'am', 'start', '-n', 'codes.t3.dev/com.longbridge.nsh.AppActivity'))
        }
        'check' {
            Initialize-Wasm
            # The GPUI shell is left out: gpui-fast (icu_segmenter 2.3) and t3-i18n's ICU 2.x dependencies do not
            # resolve together (2026-11-13; cargo backtracks t3-i18n's icu to 2.0 and fails). It is a secondary, unshipped backend; restore it here once
            # 'cargo metadata --manifest-path gpui/Cargo.toml' resolves.
            foreach ($manifest in @($uiManifest, (Join-Path $shellRoot 'desktop/Cargo.toml'))) {
                Invoke-Checked cargo @('fmt', '--manifest-path', $manifest, '--check')
                Invoke-Checked cargo @('clippy', '--locked', '--manifest-path', $manifest, '--all-targets', '--', '-D', 'warnings')
            }
            Invoke-Checked cargo @('check', '--locked', '--manifest-path', $uiManifest, '--target', 'wasm32-unknown-unknown')
        }
        'all' {
            if (-not (Test-Path $cli)) { throw 'Run ui.ps1 install-cli first' }
            Invoke-Checked cargo @('build', '--locked', '--manifest-path', (Join-Path $shellRoot 'desktop/Cargo.toml'))
            Copy-DesktopAssets 'debug'
            Invoke-Package 'web' $cli $pin
            Invoke-Package 'android' $cli $pin
            Write-Output "desktop: $(Join-Path $buildRoot 'debug/t3-desktop.exe')"
            Write-Output "web:     $(Join-Path $outRoot $appName)"
            Write-Output "android: $(Join-Path $outRoot "$appName.apk")"
        }
        default {
            if (-not (Test-Path $cli)) { throw 'Run ui.ps1 install-cli first' }
            Invoke-Package $Action $cli $pin
        }
    }
} finally {
    Pop-Location
}
