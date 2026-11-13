# Rust workspace

The Rust rewrite of T3 Code. Read the root `AGENTS.md` first; its rules apply here.
The TypeScript tree is the specification until a Rust crate replaces it: port
behaviour from the source and its tests, not from memory.

## The one rule

One change covers every platform. All UI and client logic live in shared crates
that compile for desktop, mobile, and web. A platform shell holds an entry point
and packaging, never feature code. If a change needs a second edit for another
platform, the abstraction is wrong: fix it behind a platform trait.

## Crates

- `t3-*` names, one responsibility each, `crates/<name>`.
- A crate's `lib.rs` doc comment says whether it must compile to
  `wasm32-unknown-unknown`. Shared crates (`t3-contracts`, client and UI crates)
  must: no `std::fs`, `std::net`, `std::process`, threads, or tokio runtime in them.
  Native crates (server, persistence, git, providers, terminal) use tokio.
- Declare dependencies in the crate's own `Cargo.toml` while work is in flight.
  Do not edit the workspace `Cargo.toml` from a delegated task.

## Compatibility

Rust and TypeScript run side by side during the transition, so wire and disk
formats are fixed points.

- Wire types: `node rust/tools/export/main.ts` regenerates `fixtures/` from the
  TypeScript contracts. A Rust type is done when its golden samples round-trip.
- Database: a fresh Rust-created database must equal `fixtures/database/schema.sql`,
  and Rust must open a database the TypeScript server created.
- MCP tool names and schemas, CLI flags, and on-disk file formats stay identical
  unless a maintainer says otherwise.

## Serde conventions

- Optional key (may be absent): `Option<T>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Nullable key (always present, may be `null`): plain `Option<T>`.
- Branded ids: `#[serde(transparent)]` newtypes over `String`.
- Dates stay as the exact wire string; do not reparse and reformat them.
- Unions that the TypeScript side decodes leniently keep an explicit unknown
  variant so a newer peer does not break an older one.

## Code

- Errors are `thiserror` enums per crate. No `anyhow` in libraries, no `unwrap`
  outside tests, no `unsafe` without a maintainer's sign-off.
- Plain structs with methods. Add a trait only at a real seam: provider adapter,
  forge, platform shell, clock.
- Spawn processes through the shared process crate once it exists; never kill by
  name or pattern.
- Tests assert on events and state, never on sleeps or elapsed time. Use tokio's
  paused clock for time.

## Checks

From `rust/`: `cargo fmt --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo test -p <crate>`. For shared crates also
`cargo check -p <crate> --target wasm32-unknown-unknown`. Scope checks to the
crates you changed.
