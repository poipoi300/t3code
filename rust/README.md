# Rust workspace

The Rust server and clients will replace the TypeScript implementation incrementally.
Wire and database fixtures are generated from the current TypeScript checkout, using
the installed Effect version and the server's real SQLite setup and migrations.

From the repository root (Node and the installed JS dependencies are required):

```sh
node rust/tools/export/main.ts
node rust/tools/export/main.ts --check
```

From `rust/`:

```sh
cargo test -p t3-contracts -- --nocapture
```

`fixtures/manifest.json` records schema exports, sample counts and unsupported
operations with their reasons. RPC and orchestration manifests reference
`contracts.schema.json`. Named exports retain separate entries even when Effect
erases their brands to the same runtime AST. Golden samples use seed 29747 and the
RPC JSON codec; edge candidates are accepted only by that codec. Lone UTF-16
surrogates are retained in `non-scalar/`; scalar replacements are validated again
before entering the Rust goldens. Unsupported entries distinguish JSON Schema
approximations, bounded arbitrary exhaustion, and decode-only tag aliases.
Rust tests report registered type coverage and list every unregistered schema.
Round trips check wire preservation, not equivalent input validation or behavior.

## Shared Rust UI (NSH)

From the repository root, in PowerShell 7 on Windows, with Rust 1.99+, MSVC C++
tools, Python 3 and LLVM (`clang`, `llvm-ar`) on PATH:

```powershell
pwsh -NoProfile -File rust/shells/ui.ps1 install-cli
pwsh -NoProfile -File rust/shells/ui.ps1 desktop
pwsh -NoProfile -File rust/shells/ui.ps1 gpui
pwsh -NoProfile -File rust/shells/ui.ps1 web
pwsh -NoProfile -File rust/shells/ui.ps1 serve -Port 18764
pwsh -NoProfile -File rust/shells/ui.ps1 android -Abi x86_64
pwsh -NoProfile -File rust/shells/ui.ps1 install-android -Device emulator-5554
pwsh -NoProfile -File rust/shells/ui.ps1 check
```

Build output goes to `shells/target` unless `-TargetDir` or `$env:T3_SHELLS_TARGET` names
another directory. Web output: `<target>/out/T3 Code Dev/`; APK: `<target>/out/T3 Code Dev.apk`.
`ui.ps1 all` builds desktop, web and Android in one go; see `shells/ONE-CHANGE.md`.
Android requires SDK platform 35, build-tools 35+, an NDK, and JDK 17+; set
`ANDROID_HOME`, `ANDROID_NDK_HOME` and `JAVA_HOME` if discovery is unsuitable.
Install targets an already running device; it never starts or clears an emulator.
On macOS with Xcode and PowerShell 7, use `ui.ps1 macos`, `ui.ps1 ios-simulator`
or `ui.ps1 ios` (device signing required); these are unverified on this Windows host.
See [shell ownership and build isolation](shells/README.md) for the pin, generated
runners, optional GPUI backend and platform tooling workarounds.
