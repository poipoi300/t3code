#![cfg(feature = "acp-adapter")]

use serde_json::{Value, json};
use t3_orchestration::ports::acp_replay::testkit::fixtures::index::INPUT_EXPORTS;

#[tokio::test]
async fn fixture_inputs_and_constants_equal_the_real_typescript_builders()
-> Result<(), Box<dyn std::error::Error>> {
    let requests: Vec<_> = INPUT_EXPORTS
        .iter()
        .map(|export| json!({"source":export.source,"name":export.name}))
        .collect();
    let mut options = t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import { registerHooks } from 'node:module';
import { pathToFileURL } from 'node:url';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
registerHooks({resolve(specifier, context, next) {
    if (specifier === '@effect/vitest') return {url:'data:text/javascript,export const assert={};',shortCircuit:true};
    return next(specifier,context);
}});
const requests = JSON.parse(readFileSync(0,'utf8'));
const results = [];
for (const {source,name} of requests) {
    const module = await import(pathToFileURL(resolve(source)).href);
    const exported = module[name];
    results.push({source,name,value:typeof exported === 'function' ? exported() : exported});
}

process.stdout.write(JSON.stringify(results));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    let output = t3_process::run(options).await?;
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(actual.len(), INPUT_EXPORTS.len());
    for (export, actual) in INPUT_EXPORTS.iter().zip(actual) {
        let expected =
            json!({"source":export.source,"name":export.name,"value":(export.evaluate)()?});
        assert_eq!(actual, expected, "{}::{}", export.source, export.name);
    }
    Ok(())
}

#[tokio::test]
async fn seeded_fixture_uuids_equal_effect_random_across_generation_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::ports::acp_replay::testkit::deterministic_runtime::SeededIds;
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const require=createRequire(resolve('packages/provider-core/package.json'));
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Random=await import(pathToFileURL(require.resolve('effect/Random')).href);
const {randomUuidV4}=await import(pathToFileURL(resolve('packages/provider-core/src/server/randomUuid.ts')).href);
const results=[];
for (const seed of [0,1,0x12345678,0xffffffff]) {
    results.push({seed,uuids:await Effect.runPromise(Effect.all(Array.from({length:100},()=>randomUuidV4)).pipe(Random.withSeed(seed)))});
}
process.stdout.write(JSON.stringify(results));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let output = t3_process::run(options).await?;
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    for row in rows {
        let ids = SeededIds::new(row["seed"].as_u64().unwrap() as u32);
        let actual: Vec<_> = (0..100).map(|_| ids.uuid_v4()).collect();
        assert_eq!(json!(actual), row["uuids"], "seed={}", row["seed"]);
    }
    Ok(())
}

#[tokio::test]
async fn every_fixture_materializes_the_same_commands_and_barriers_as_typescript()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::ports::acp_replay::testkit::{
        deterministic_runtime::{DEFAULT_RANDOM_SEED, SeededIds},
        fixtures::{index::INPUT_BUILDERS, shared::materialize_fixture_input},
    };
    let requests: Vec<_> = INPUT_BUILDERS
        .iter()
        .map(|(source, name, _)| json!({"source":source,"name":name}))
        .collect();
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';
import {readFileSync} from 'node:fs';
registerHooks({resolve(specifier,context,next) {
    if(specifier==='@effect/vitest')return {url:'data:text/javascript,export const assert={};',shortCircuit:true};
    return next(specifier,context);
}});
const require=createRequire(resolve('packages/provider-core/package.json'));
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Random=await import(pathToFileURL(require.resolve('effect/Random')).href);
const IdAllocator=await import(pathToFileURL(resolve('packages/provider-core/src/server/IdAllocator.ts')).href);
const {materializeFixtureInput}=await import(pathToFileURL(resolve('apps/server/src/orchestration-v2/testkit/fixtures/shared.ts')).href);
const result=[];
for(const {source,name} of JSON.parse(readFileSync(0,'utf8'))) {
    const module=await import(pathToFileURL(resolve(source)).href);
    result.push(await Effect.runPromise(materializeFixtureInput({scenario:name,fixtureInput:module[name](),driver:'cursor',modelSelection:{instanceId:'cursor',model:'composer-2.5'}}).pipe(Effect.provide(IdAllocator.layer),Random.withSeed(0x12345678))));
}
process.stdout.write(JSON.stringify(result));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), INPUT_BUILDERS.len());
    let model = serde_json::from_value(json!({"instanceId":"cursor","model":"composer-2.5"}))?;
    for ((source, name, builder), expected) in INPUT_BUILDERS.iter().zip(expected) {
        let actual = materialize_fixture_input(
            name,
            &builder()?,
            "cursor",
            &model,
            &SeededIds::new(DEFAULT_RANDOM_SEED),
        )?;
        assert_eq!(serde_json::to_value(&actual)?, expected, "{source}::{name}");
        actual.into_scenario(*name)?;
    }
    Ok(())
}

#[tokio::test]
async fn runtime_instruction_rewriting_equals_typescript_for_all_recordings()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::{
        ports::acp_replay::testkit::{
            fixtures::index::RECORDED_FIXTURES,
            replay_runtime_instructions::materialize_replay_transcript_runtime_instructions,
        },
        testing::replay::decode_ndjson,
    };
    let requests:Vec<_>=RECORDED_FIXTURES.iter().map(|recording| {
        let transcript=decode_ndjson(recording.ndjson).unwrap();
        json!({"transcript":transcript,"runtime":{"driver":transcript.provider,"model":"fixture-model"}})
    }).collect();
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {registerHooks} from 'node:module';
import {readFileSync} from 'node:fs';
registerHooks({load(url,context,next) {
    if(url.endsWith('/provider-core/src/server/runtimeInstructions.ts'))return {format:'module',source:'export const buildRuntimeInstructions=({harness,model})=>`${harness}|${model}`;',shortCircuit:true};
    return next(url,context);
}});
const {materializeReplayTranscriptRuntimeInstructions:rewrite}=await import('./apps/server/src/orchestration-v2/testkit/ReplayRuntimeInstructions.ts');
const outbound=transcript=>transcript.entries.filter(entry=>entry.type==='expect_outbound');
process.stdout.write(JSON.stringify(JSON.parse(readFileSync(0,'utf8')).map(({transcript,runtime})=>outbound(rewrite(transcript,runtime)))));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    options.max_output_bytes = 32 * 1024 * 1024;
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), RECORDED_FIXTURES.len());
    for (recording, expected) in RECORDED_FIXTURES.iter().zip(expected) {
        let transcript = decode_ndjson(recording.ndjson)?;
        let driver = transcript.provider.0.clone();
        let actual = materialize_replay_transcript_runtime_instructions(
            transcript,
            &driver,
            "fixture-model",
            |harness, model| format!("{harness}|{model}"),
        );
        let actual: Vec<_> = actual
            .entries
            .into_iter()
            .filter(|entry| {
                matches!(
                    entry,
                    t3_orchestration::testing::replay::ReplayEntry::ExpectOutbound(_)
                )
            })
            .collect();
        assert_eq!(json!(actual), expected, "{}", recording.file);
    }
    Ok(())
}

#[tokio::test]
async fn pull_request_shell_fixtures_equal_typescript_with_offsets_and_optional_links()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::ports::acp_replay::testkit::pull_request_fixtures::*;
    let base = json!({"id":"thread:fixture","projectId":"project:fixture","title":"fixture","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"pullRequests":[],"latestUserMessageAt":null,"createdAt":"2026-11-13T10:20:30.123-05:00","updatedAt":"2026-11-13T15:20:30Z","archivedAt":null,"settledOverride":null,"settledAt":null});
    let mut linked = base.clone();
    linked["linkedPullRequest"] = Value::Null;
    linked["branchPullRequest"] = Value::Null;
    linked["latestUserMessageAt"] = json!("2026-11-12");
    linked["lineage"] = json!({"rootThreadId":"thread:parent","parentThreadId":"thread:parent","relationshipToParent":"fork"});
    let requests = vec![base, linked];
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {readFileSync} from 'node:fs';
const {v2PullRequestThread}=await import('./apps/server/src/orchestration-v2/testkit/pullRequestFixtures.ts');
process.stdout.write(JSON.stringify(JSON.parse(readFileSync(0,'utf8')).map(v2PullRequestThread)));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    for (input, expected) in requests.into_iter().zip(expected) {
        assert_eq!(
            serde_json::to_value(v2_pull_request_thread(serde_json::from_value(input)?)?)?,
            expected
        );
    }
    Ok(())
}

#[tokio::test]
async fn fixture_registry_metadata_equals_all_typescript_provider_variants()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::ports::acp_replay::testkit::{
        fixtures::index::replay_fixture_definitions, provider_replay_fixtures::fixture_cases,
    };
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {registerHooks} from 'node:module';
registerHooks({resolve(s,c,next){if(s==='@effect/vitest')return {url:'data:text/javascript,export const assert={};',shortCircuit:true};return next(s,c);}});
const {ORCHESTRATOR_REPLAY_FIXTURES:fixtures}=await import('./apps/server/src/orchestration-v2/testkit/fixtures/index.ts');
process.stdout.write(JSON.stringify(fixtures,(key,value)=>typeof value==='function'?value.name:key==='transcriptFile'?String(value).split('/fixtures/')[1]:value));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let output = t3_process::run(options).await?;
    let expected: Value = serde_json::from_slice(&output.stdout)?;
    let actual = replay_fixture_definitions()?;
    assert_eq!(actual.len(), 97);
    assert_eq!(
        actual
            .iter()
            .map(|fixture| fixture.providers.len())
            .sum::<usize>(),
        160
    );
    assert_eq!(serde_json::to_value(actual)?, expected);
    assert_eq!(fixture_cases("cursor")?.len(), 10);
    Ok(())
}
