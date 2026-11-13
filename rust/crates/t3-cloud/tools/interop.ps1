param()
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    throw 'Set CARGO_TARGET_DIR to the assigned build directory before running interoperability checks.'
}
$repository = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$temporary = [IO.Path]::GetFullPath((Join-Path $temporaryRoot ('t3-cloud-interop-' + [guid]::NewGuid().ToString('N'))))
[IO.Directory]::CreateDirectory($temporary) | Out-Null
Push-Location (Join-Path $repository 'rust')
try {
    & node (Join-Path $PSScriptRoot 'interop.ts') prepare $temporary
    if ($LASTEXITCODE -ne 0) { throw 'TypeScript proof creation failed.' }
    & cargo run -q -p t3-cloud --example interop -- $temporary
    if ($LASTEXITCODE -ne 0) { throw 'Rust interoperability check failed.' }
    & node (Join-Path $PSScriptRoot 'interop.ts') verify $temporary
    if ($LASTEXITCODE -ne 0) { throw 'TypeScript response verification failed.' }
}
finally {
    Pop-Location
    $target = Get-Item -LiteralPath $temporary
    if ($target.Parent.FullName -ne $temporaryRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) -or
        !$target.Name.StartsWith('t3-cloud-interop-', [StringComparison]::Ordinal)) {
        throw 'Refusing to remove an unexpected interoperability directory.'
    }
    Remove-Item -LiteralPath $target.FullName -Recurse -Force
}
