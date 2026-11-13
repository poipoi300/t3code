# Client shells

All screens, state, tokens and primitives live in `../crates/t3-ui`. Shells only
register that crate and start a backend. `desktop` selects Win32 on Windows,
AppKit on macOS and GTK on Linux. `gpui` is a separate embedding entry point:
NSH's CLI does not package GPUI Rust apps, and its dependency graph should not
be enabled by a normal native build. `ui.ps1` fills the checked-in desktop
`Cargo.toml.in` templates from the shared crate's pin; the resulting manifests
are ignored. Only the templates and entry points are hand maintained.

`nsh.json` is the packaging entry for **web (HTML/wasm), Android and iOS**, and
native desktop bundles. The pinned CLI owns the runners it generates below
`.nsh/rust/rust-{web,android,ios,windows,macos,linux}`. Regenerate those runners;
do not commit or edit them, or make junctions to them. In particular, iOS uses
the CLI's UIKit runner and platform component registration, not a second UI.
`ui.ps1 ios-simulator` on macOS builds a simulator `.app`; `ui.ps1 ios` needs
the signing options described by `target/tools/bin/nsh package --help` for a
distributable IPA. Neither Apple build has been verified on Windows.

The exact Git pin lives only in `t3-ui/Cargo.toml`; `ui.ps1 install-cli` reads
and validates that all NSH dependency declarations agree, then installs the
same revision with `cargo install --git ... --rev ... --locked`. Version 0.5.0
is published, but this later pin includes Rust app/Windows build fixes and
the fix keeping native engine features out of the web runtime. Review those
constraints before moving back to a release. The CLI has JS tooling; application
dependency graphs must contain no `nsh-js` or QuickJS. Browser DOM glue is supplied
by NSH; application code remains Rust.

UI and the two handwritten shells have standalone Cargo workspaces and committed
lockfiles. The server workspace excludes them, so server package tests neither
resolve nor compile UI dependencies. `.nsh/rust` is a disposable CLI-owned workspace
with feature unification for the describe tool and runners. Build output is shared
in `shells/target` by default, independently of `rust/target`; `-TargetDir` or
`$env:T3_SHELLS_TARGET` moves it (and the CLI, the runtime and the packages under its `out`)
elsewhere, for example to another drive. Generated manifests may use
absolute paths into Cargo's pinned Git cache; no sibling source checkout is used.
`packaging.Cargo.lock` seeds a clean generated workspace's transitive resolution;
the CLI may update its feature-unification entries while building a target. Refresh
the seed deliberately after a dependency upgrade, not after every build.

`ui.ps1` sets `RUST_MIN_STACK=268435456` for Windows compilation. Web builds select
LLVM clang/llvm-ar for wasm C dependencies; the generated web runner sets an 8 MiB
wasm stack. Android builds compile Java sources from Cargo's pinned NSH checkout
with `javac --release 17` and SDK `d8`, staging `classes.dex` and ABI enumeration
directories under `target/runtime`. `NSH_RUNTIME` points there, avoiding NSH's
source-layout discovery assumptions after a Cargo install. The CLI builds the
actual Rust `libnsh.so`; the ABI directory is not a prebuilt application runtime.

`ui.ps1 all` builds the desktop executable, the web site and the APK from the same source and
prints their paths; `capture.ps1` drives the slice on each target and saves screenshots
([ONE-CHANGE.md](ONE-CHANGE.md) is the procedure). The desktop templates enable NSH's `svg`
feature (icons are SVG assets).

The GPUI shell does not resolve today (`gpui-fast`, through `icu_segmenter` 2.3, and `t3-i18n`'s ICU 2.x
dependencies do not resolve together); `check` leaves it out until that is fixed.

`ui.ps1 check` formats/checks only handwritten crates, runs Clippy with warnings
denied, and checks the shared crate for wasm. CLI-generated runners are excluded
from formatting/Clippy because regeneration owns them, but packaging compiles them.

On macOS, after installing the CLI, the source-inspected Apple commands are:

```powershell
pwsh -NoProfile -File rust/shells/ui.ps1 macos -Run
pwsh -NoProfile -File rust/shells/ui.ps1 ios-simulator -Run
pwsh -NoProfile -File rust/shells/ui.ps1 ios -SigningIdentity 'Apple Development: NAME (TEAM)' -ProvisioningProfile '/path/Dev.mobileprovision'
```

They require Xcode, the matching Apple Rust targets and (for devices) your signing
identity/profile. Native `desktop` also selects AppKit on macOS; GPUI's macOS
build additionally needs the Metal toolchain. Apple compilation, signing,
installation and rendering are unverified here. Linux/GTK is source-inspected only.
