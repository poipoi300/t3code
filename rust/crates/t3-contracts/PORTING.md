# Contract porting

This crate is shared by native and wasm32-unknown-unknown clients. Keep library
code free of filesystem, networking, processes, threads, and runtime dependencies.

Each TypeScript source file owns `src/<snake_case>.rs` and
`tests/golden/<snake_case>.rs`. All 78 files, including the index barrel, are wired
already. Fill only your assigned pair. Do not edit lib.rs or golden/mod.rs.
Public types live under their owning modules; no new root or index re-exports.
The original eight root exports and WireDateTime remain for compatibility.

Read packages/contracts/src/<file>.ts as the specification. These are encoded
wire representations, not Effect's decoded application models. Derive serde
structs/enums; do not replace a contract with Value just to raise coverage.
Use Value for genuinely unknown/opaque payloads, including Defect causes.
Golden equality establishes wire preservation, not complete filter validation.

Find names in fixtures/manifest.json's exports (`file.ts#Export -> schema name`).
The manifest already carries source ownership. tests/schema_modules.json resolves
re-exports to declarations and assigns synthetic union members to their parent
and `rpc.*` names to rpc. It covers all 2,216 sample names; refresh it only when
fixtures change. BrowserRecordingFrameRate belongs to preview, not settings.
Samples are JSONL at fixtures/<samples[].file>; count must equal samples[].count.

In your registration file, import your types and `super::super::register`, then
add `register::<YourType>(registry, "ExactSchemaName");` to register_module.
Aliases have separate manifest names and need separate entries. Synthetic member
names can reuse the correct union type. The runner rejects duplicate names and
registrations owned by another module. Empty registrations do not mean coverage.

Serde fields use camelCase unless the source explicitly spells another name.
Keep `_tag` explicit. For the following field attributes, use crate::wire:
- optionalKey(T): Option<T>; default, skip_serializing_if="Option::is_none",
  with="crate::wire::optional". A null is rejected unless T itself accepts it.
- NullOr(T): Option<T>; with="crate::wire::nullable", WITHOUT default or skip.
- optional(T): Option<Option<T>>; default, skip_serializing_if="Option::is_none",
  with="crate::wire::double_option". None is absent; Some(None) is TypeScript's
  undefined (null on the RPC wire, absent when stored); Some(Some(v)) is a value.
- optional(NullOr(T)), optionalKey(NullOr(T)), optional(Unknown): the same
  Option<Option<T>> with with="crate::wire::nullable_optional": Some(None) is a
  stored null too. Picking the wrong one fails tests/golden.rs `stored_dialect`.
- ForwardCompatibleOptional/Nullable: with="crate::wire::lenient"; optional
  fields also default and skip. OmittedWhenNull uses that optional form.
Use deserialize_lenient_array only for ForwardCompatibleArray. For tagged unions,
implement TaggedUnion on a strict known enum and wrap it in ForwardCompatible<K>.
Unknown tags decode to UnknownUnionMember and cannot encode; malformed known tags
must fail. Use serde alias for legacy tag decodes and rename for canonical encodes.
deserialize_union_array drops unknown tags but still rejects bad known
members. ChatUnknownAttachment is a different, explicitly encodable fallback.

Use string_id! for transparent branded String newtypes. Decoding applies the
TypeScript refinement (`wire::normalize_id`: trimmed, non-blank, slug/day
patterns); `From<String>`/`From<&str>` stay unchecked for code that builds ids,
and `Type::checked` validates. TrimmedString/TrimmedNonEmptyString trim with
ECMAScript whitespace on decode and offer the same `From`/`checked` pair.
WireDateTime / base_schemas::IsoDateTime preserve strings verbatim, including empty
strings and extended years. IsoDateTime itself is only Schema.String in TS.
NonNegativeInt/PositiveInt use u64, Int uses i64, PortSchema uses u16; samples
include 9,007,199,254,740,991. General Number/Finite uses wire::JsNumber: the
IEEE double JavaScript holds, written exactly as JSON.stringify writes it
(`1`, `0.000001`, `1e+21`), rejecting non-finite input. A plain f64 field writes
`1.0` for 1, so never use f64 for a TypeScript number.

## Two JSON dialects

TypeScript encodes the same schema two ways. Use the matching Rust entry point:
- RPC dialect, `Schema.toCodecJson`: WebSocket RPC (Effect RPC `codecFor`), the
  HTTP API, guest HTTP, MCP tool input, client caches. Use plain serde_json
  (`serde_json::to_string`/`to_value`). Undefined optional values are `null`.
- Stored dialect, the plain codec (`Schema.decodeUnknown*`/`fromJsonString`):
  the event store and event metadata (OrchestrationEventStore.ts), projection
  JSON columns (ProjectionStore.ts), auth scopes, relay runtime config, review
  guides, settings.json and keybindings.json. Use `t3_contracts::stored::{to_string,
  to_writer, to_value}`; an undefined optional key is omitted, because the
  TypeScript decoder rejects `null` there. Decoding is the same serde Deserialize.
`fixtures/stored/` holds stored-dialect samples (manifest `stored`, labelled by
boundary). `stored_dialect` checks TypeScript to Rust. For Rust to TypeScript run
`T3_CONTRACT_STORED_OUT=<tmp>/stored.jsonl cargo test -p t3-contracts --test golden
stored_dialect`, then from the repository root `node rust/tools/export/verifyStored.ts
<tmp>/stored.jsonl`, which runs the real store decoder over every Rust row.
Serde writes an internally tagged union's tag first; TypeScript writes keys in
declaration order. Stores that compare bytes must order keys themselves.

## serde_json features

The crate declares `float_roundtrip` (exact float parsing), `preserve_order`
(`Value` payloads keep insertion order) and `raw_value` (JsNumber's exact text).
Never enable `arbitrary_precision` in a crate that links this one: serde then
buffers fractional numbers as a private map, and every f64 inside an internally
tagged or untagged enum stops decoding (`wire` test
`buffered_unions_decode_fractional_numbers` fails in such a build).

## Secrets

A type holding a token, ticket, code, proof, nonce, credential or password gets a
redacting Debug: `crate::redacted_debug!(Type { shown, .. } redact { secret })`.
`tests/redaction.rs` loads each with a marker secret and scans `src/` for secret
named string fields without one.

orchestration_v2_id_table provides encode/decode_record_id_table with TopLevel,
Key, or NoRecords containers. Encode from the typed envelope to retain field
order. Decode consumes ids once and rejects bad references, even optional ones.
Wire structs keep compact ids/indices (IdOrIndex<T>); expansion changes the wire
form. TypeScript allows indices only inside the three compact envelopes; the
entity types still accept them in Rust (review finding H2, open). Stored provider environment variables retain the wire secret + empty value;
application-side secret resolution is separate. Legacy model options/selections
normalize on decode; do not use aliases that change canonical encoding.

Manifest unsupported entries are per operation: JSON Schema approximations do
NOT waive available golden samples. Read the TS filters/transforms too. Sampling
exhaustion may leave only deterministic edges. Decode-only legacy aliases require
normalization tests, not round-trip equality. Lone UTF-16 surrogates remain in
fixtures/non-scalar: serde_json cannot represent them; never silently replace them.

From rust/, run one module: cargo test -p t3-contracts --test golden golden::model::
Coverage: cargo test -p t3-contracts --test golden coverage -- --nocapture
(asserts that only ACP groups are unregistered). Differential parity:
`differential` replays fixtures/differential.jsonl (mutated samples judged by
the real TypeScript decoder); tests/differential_gaps.txt lists known gaps to fix.
Final checks: cargo fmt -p t3-contracts --check; cargo test -p t3-contracts;
cargo clippy -p t3-contracts --all-targets -- -D warnings;
cargo check -p t3-contracts --target wasm32-unknown-unknown.
Scope every Cargo command with -p t3-contracts; the build lock is shared.
Do not touch other crates, workspace Cargo.toml, fixtures, tools, live T3 state,
or another engineer's modules. Do not commit or push from an implementation task.
