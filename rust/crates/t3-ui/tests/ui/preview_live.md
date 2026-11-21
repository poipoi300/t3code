# Live preview annotation tests

`prv_2::annotation_highlights_live_viewer_at_{desktop,phone}_width` are `#[ignore]`d: they need a
real `t3-server`, its preview worker and Node. `preview_live.py` starts a private server (own base
directory, port 0, no browser, a whitelisted environment without `DISPLAY` or `WAYLAND_DISPLAY`),
swaps Chromium for the worker fixture's CDP stub, writes the pairing URL to a mode-0600 file, runs
the two tests, and stops the server.

Build the server and the `ui` test binary, then from `rust/crates/t3-ui`:

```sh
. /mnt/windows/t3code-rust-build/env.sh <lane>
cargo build -p t3-server          # from rust/
cargo test --test ui --no-run     # prints the path of the ui-<hash> binary
bash /mnt/windows/t3code-rust-build/access/secret-service.sh \
  python3 tests/ui/preview_live.py \
    --server <target>/debug/t3-server --tests <target>/debug/deps/ui-<hash> --scratch <private tmp dir>
```

The exit code is the larger of the two test exit codes. The script writes one row into the server's
`orchestration_v2_projection_threads` table, so it follows that schema; update the insert when the
table changes.
