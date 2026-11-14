# Apple CI scripts

The steps of `.github/workflows/rust-rewrite.yml`, runnable on a Mac with Xcode, PowerShell 7 and
the Rust toolchain `rust/rust-toolchain.toml` pins. Run them from the repository root with `bash`.
Each script prints its usage when called without arguments.

| Script | What it does |
|---|---|
| `server.sh` | Starts `t3 serve` in dev mode (the free Replay provider) on a scratch home, seeds a "Replay showcase" thread, mints pairing links, stops the server by its captured PID. Also runs under Git Bash on Windows. |
| `macos-desktop.sh` | Starts the desktop shell, waits until OCR reads "Connect to T3 Code" in its window, captures it; with `--pair-url`, pairs through the app's form and captures the thread list. |
| `ios-sim.sh` | Boots an iPhone simulator, installs and launches the simulator app, waits until OCR reads the connect screen, captures it. |
| `one-change.sh` | `apply` makes the one-line change in `rust/crates/t3-ui` and fails if anything else changed; `verify` checks two OCR readings; `revert` undoes it. |
| `apple-probe.swift` | The window lookup and the OCR (Vision) the scripts use, compiled on first use into `$T3_CI_BIN`. |

The same proof on a Mac, before and after the change:

```sh
export T3_SHELLS_TARGET="$PWD/rust/shells/target"
(cd rust && cargo build --locked -p t3-server --bin t3)
(cd rust/crates/t3-ui && CARGO_TARGET_DIR="$T3_SHELLS_TARGET" cargo build --locked --example seed_thread)
pwsh -NoProfile -File rust/shells/ui.ps1 desktop-build
pwsh -NoProfile -File rust/shells/ui.ps1 install-cli
pwsh -NoProfile -File rust/shells/ui.ps1 ios-sim
bash rust/tools/ci/server.sh start /tmp/t3-run --t3 rust/target/debug/t3
bash rust/tools/ci/server.sh seed /tmp/t3-run --seed "$T3_SHELLS_TARGET/debug/examples/seed_thread"
bash rust/tools/ci/macos-desktop.sh --app "$T3_SHELLS_TARGET/debug/t3-desktop" --out /tmp/t3-captures \
  --label before --pair-url "$(bash rust/tools/ci/server.sh pair /tmp/t3-run)"
bash rust/tools/ci/ios-sim.sh --app "$T3_SHELLS_TARGET/out/T3 Code Dev.app" --out /tmp/t3-captures --label before
bash rust/tools/ci/one-change.sh apply   # then rebuild both and capture with --label after
bash rust/tools/ci/server.sh stop /tmp/t3-run
```

Pairing types into the app through System Events, so the terminal running the script needs the
Accessibility permission, and window captures need Screen Recording. The desktop app keeps its data
under the capture directory, but its storage key goes to the login keychain as
"T3 Code client storage"; delete that item afterwards if you do not want it.
