# Reference captures of the real app

The Rust UI's "done" check compares each Rust screen with the TypeScript app's on the same data
and viewport. `reference/ui-reference.e2e.ts` produces the TypeScript side through the repo's
scenario harness (`docs/operations/scenarios.md`): it seeds the DemoBackend's projects, threads
and messages through the app's own CLI and the dev replay agent, then captures every route at
1280x800 and 390x844. A t3-ui test (`the_reference_scenario_seeds_the_same_projects_threads_and_messages`)
fails when the scenario and `fixtures.rs` drift apart, and another
(`replies_are_what_the_replay_recordings_play`) checks the fixture replies against the replay
transcripts.

## Run once (maintainer or coordinator)

The harness starts the TypeScript dev stack on its own home (`.t3/scenarios`), never `~/.t3`.
From the worktree root, in Git Bash, with the dev setup installed (`vp i`):

```sh
cp rust/shells/reference/ui-reference.e2e.ts scripts/scenarios/rust-ui-reference.e2e.ts
touch .e2e-reference-stamp
(cd scripts && E2E_TELEMETRY_DISABLED=1 ./node_modules/.bin/e2e run scenarios/rust-ui-reference.e2e.ts --target web)
mkdir -p ../t3code-rust-notes/one-change/reference
find scripts/.e2e -path '*screenshots*' -name '*.png' -newer .e2e-reference-stamp \
  -exec cp {} ../t3code-rust-notes/one-change/reference/ \;
rm scripts/scenarios/rust-ui-reference.e2e.ts .e2e-reference-stamp
```

Captures are named `<desktop|phone>-<route>.png` (`desktop-thread-showcase.png`,
`phone-settings-appearance.png`). Re-run when the web app changes a ported screen.

## Limits

- The phone size is a 390x844 viewport with a fine pointer; `pointer-coarse:` styles show only on
  the Android capture of the Rust app and in the mobile app.
- Mobile-only pages need the iOS target (`--target ios`), which leases a simulator from the
  Simlock pool (`SIMLOCK_AGENT_DEVICE_URL`, `AGENT_DEVICE_DAEMON_AUTH_TOKEN`). The slice has none.
- Seeded threads are seconds old, so the web shows `now` where the demo's fixed timestamps show
  hours; the showcase thread also shows work rows (plan, tool calls, approval) the slice does
  not draw yet.
- Routes that need ids the scenario does not create (`/draft/$draftId`, `/projects/$projectKey`)
  are left out.
