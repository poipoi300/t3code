$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    throw "Set CARGO_TARGET_DIR to the assigned build directory before running interoperability checks."
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../../../..")).Path
$run = Join-Path ([System.IO.Path]::GetTempPath()) ("t3-auth-interop-" + [guid]::NewGuid())
try {
    & node (Join-Path $PSScriptRoot "interop.ts") prepare $run
    if ($LASTEXITCODE -ne 0) { throw "TypeScript prepare failed" }
    & cargo run --manifest-path (Join-Path $repo "rust/Cargo.toml") -p t3-auth --example interop -- $run
    if ($LASTEXITCODE -ne 0) { throw "Rust verification failed" }
    & node (Join-Path $PSScriptRoot "interop.ts") verify $run
    if ($LASTEXITCODE -ne 0) { throw "TypeScript verification failed" }
    & cargo run --manifest-path (Join-Path $repo "rust/Cargo.toml") -p t3-auth --example interop -- $run recheck
    if ($LASTEXITCODE -ne 0) { throw "Rust replay recheck failed" }
    & node (Join-Path $PSScriptRoot "interop.ts") finish $run
    if ($LASTEXITCODE -ne 0) { throw "TypeScript replay recheck failed" }
    Write-Output "Bidirectional TypeScript/Rust authentication interop passed."
} finally {
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    $resolvedRun = [System.IO.Path]::GetFullPath($run)
    if (!$resolvedRun.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [System.IO.Path]::GetFileName($resolvedRun) -notmatch '^t3-auth-interop-[0-9a-f-]{36}$') {
        throw "Refusing to remove an unexpected interop directory"
    }
    if (Test-Path -LiteralPath $resolvedRun) { Remove-Item -LiteralPath $resolvedRun -Recurse -Force }
}
