# Proving one change reaches every platform

The rule: a change to `rust/crates/t3-ui` reaches the Windows desktop app, the web build and the
Android app with no other edit. This is how to show it, from PowerShell 7 on Windows. Keep build
output off the worktree's drive if space is short: every command below honours `-TargetDir` or
`$env:T3_SHELLS_TARGET`.

```powershell
$env:T3_SHELLS_TARGET = 'D:\t3code-rust-target\shells'
pwsh -NoProfile -File rust/shells/ui.ps1 install-cli      # once per NSH pin
pwsh -NoProfile -File rust/shells/ui.ps1 all              # desktop exe, web site, APK
```

1. **Before.** Run `ui.ps1 all`. Start an Android device or emulator (`adb devices` lists it), then
   capture:

   ```powershell
   $out = 'C:\path\to\captures'
   pwsh -NoProfile -File rust/shells/capture.ps1 desktop -Out $out -Label before
   pwsh -NoProfile -File rust/shells/capture.ps1 web     -Out $out -Label before
   pwsh -NoProfile -File rust/shells/capture.ps1 android -Out $out -Label before -Device emulator-5554
   ```

   Each capture pairs with the demo server through the real pairing screen, captures the thread
   list, opens the first thread and captures it. Web captures run at 1280x800 and at a 390x844
   phone width.

2. **Change one thing** in `rust/crates/t3-ui` (a shared component or a screen), nothing else.

3. **After.** Run `ui.ps1 all` again and the same three captures with `-Label after`.

4. **Show the change set:** `git status --short rust/` and `git diff --stat` between the two runs
   must name only files under `rust/crates/t3-ui`.

The desktop shell (`desktop/src/main.rs`), the generated web and Android runners and `nsh.json` are
not touched by a feature change; if one had to be, the abstraction would be wrong.

**When sibling crates are mid-edit.** `t3-ui` builds against the live `t3-client`, `t3-theme` and
`t3-i18n`; while another engineer has one of them broken, no target builds. Copy `rust/` (without
`target` directories, `shells/.nsh` and `fixtures`) to a scratch directory at a moment
`cargo check` passes in `crates/t3-ui`, and run that copy's `shells/ui.ps1` (captures still run
from the worktree's `capture.ps1` with the same `-TargetDir`: the web capture needs the repo's
`node_modules`);
make the one change in the worktree and copy `crates/t3-ui/src` into the scratch copy before the
second build. Say in the report which snapshot time the siblings came from.

iOS and macOS: `ui.ps1 ios-simulator`, `ui.ps1 ios` and `ui.ps1 macos` build the same crate with
the CLI's UIKit and AppKit runners on a Mac with Xcode; they have not been run on this Windows host.
