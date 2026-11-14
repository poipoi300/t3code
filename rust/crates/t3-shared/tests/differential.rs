#![allow(clippy::unwrap_used)]
#![cfg(not(target_arch = "wasm32"))]
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, process::Command};
use t3_shared::{
    find_text::*, palette_search as p, search_policy::*, terminal_history as h,
    terminal_output as t, thread_search as f,
};

fn ranges(ranges: &[p::TextRange]) -> Value {
    json!(
        ranges
            .iter()
            .map(|r| json!({"start":r.start,"end":r.end}))
            .collect::<Vec<_>>()
    )
}
fn matched(m: Option<p::SearchMatch>) -> Value {
    m.map_or(Value::Null, |m| {
        let mut result=json!({"score":m.score,"quality":m.quality,"wholeWords":m.whole_words,"strong":m.strong,"ranges":ranges(&m.ranges)});
        if let Some(v)=m.group_quality { result["groupQuality"]=json!(v); }
        if let Some(v)=m.picked_at { result["pickedAt"]=json!(v); }
        if m.picked_exactly { result["pickedExactly"]=json!(true); }
        if let Some(h)=m.hint { result["hint"]=json!({"label":h.label,"text":h.text,"ranges":ranges(&h.ranges)}); }
        result
    })
}
fn equivalent(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            (a - b).abs() <= 1e-10 * a.abs().max(b.abs()).max(1.0)
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && b.iter()
                    .all(|(k, v)| equivalent(a.get(k).unwrap_or(&Value::Null), v))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        _ => actual == expected,
    }
}
#[test]
fn current_typescript_suites_and_generated_inputs() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.ancestors().nth(3).expect("repository root");
    let output = Command::new("node")
        .arg(manifest.join("tools/typescript-oracle.mjs"))
        .current_dir(repo)
        .output()
        .expect("Node is required for current TypeScript parity");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let corpus: Value = serde_json::from_slice(&output.stdout).expect("oracle JSON");
    let mut states = HashMap::from([(0, t::TerminalOutputState::default())]);
    let mut feeds = HashMap::<usize, (usize, &str)>::new();
    let mut incremental = t::TerminalOutput::default();
    let mut incremental_id = 0;
    let mut incremental_max = 65536;
    let mut failures = vec![];
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let policy = NativeThreadSearchPolicy::default();
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        *counts.entry(name.into()).or_default() += 1;
        let args = &case["args"];
        let s = |i: usize| args[i].as_str().unwrap_or_default();
        let n = |i: usize, default: u64| args[i].as_u64().unwrap_or(default) as usize;
        let actual=match name {
            "append"=>{
                let input = n(0,0);
                let max = n(2,65536);
                if input != incremental_id || max != incremental_max {
                    incremental = t::TerminalOutput::new(max);
                    let mut previous = input;
                    let mut replay = vec![];
                    while previous != 0 {
                        let (parent, chunk) = feeds[&previous];
                        replay.push(chunk);
                        previous = parent;
                    }
                    for chunk in replay.into_iter().rev() { incremental.push(chunk); }
                }
                incremental.push(s(1));
                incremental_id = case["id"].as_u64().unwrap() as usize;
                incremental_max = max;
                feeds.insert(incremental_id,(input,s(1)));
                let snapshot = incremental.snapshot();
                let snapshot = json!({"text":snapshot.text,"pending":snapshot.pending,"truncated":snapshot.truncated});
                if !equivalent(&snapshot,&case["expected"]) && failures.len()<40 { failures.push(format!("incremental append: {}\nargs={args}\nactual={snapshot}\nexpected={}",case["test"],case["expected"])); }
                let state=t::append_terminal_output(&states[&n(0,0)],s(1),n(2,65536));
                let actual=json!({"text":state.text,"pending":state.pending,"truncated":state.truncated});
                states.insert(case["id"].as_u64().unwrap() as usize,state);
                actual
            },
            "utf8Feed"=>{
                let mut output=t::TerminalOutput::default();
                for chunk in args[0].as_array().unwrap(){output.push_bytes(&chunk.as_array().unwrap().iter().map(|b|b.as_u64().unwrap() as u8).collect::<Vec<_>>());}
                output.finish_bytes();
                let state=output.snapshot();
                json!({"text":state.text,"pending":state.pending,"truncated":state.truncated})
            },
            "resume"=>json!(t::terminal_output_resume_text(&states[&n(0,0)])),
            "tail"=>json!(t::terminal_output_tail_units(&args[0].as_array().unwrap().iter().map(|u|u.as_u64().unwrap() as u16).collect::<Vec<_>>(),n(1,65536))),
            "normalize"=>json!(t::normalize_terminal_output(s(0),n(1,65536))),
            "plain"=>json!(t::terminal_output_plain_text(s(0))),
            "spans"=>{
                let palette:Vec<_>=args[1].as_array().unwrap().iter().map(|v|v.as_str().unwrap()).collect();
                json!(t::terminal_output_spans(s(0),palette.as_slice().try_into().unwrap(),n(2,2000)))
            },
            "terminalSequenceStateAfter"=>{
                let units:Vec<_>=args[1].as_array().unwrap().iter().map(|n|n.as_u64().unwrap() as u16).collect();
                json!(h::terminal_sequence_state_after_units(n(0,0) as u32,&units,n(2,0),n(3,units.len() as u64)))
            },
            "terminalSequenceEnd"=>json!(h::terminal_sequence_end_units(n(0,0) as u32,&args[1].as_array().unwrap().iter().map(|n|n.as_u64().unwrap() as u16).collect::<Vec<_>>())),
            "retainedTerminalHistory"=>json!(h::retained_terminal_history(s(0),n(1,0))),
            "countThreadSearchOccurrences"=>json!(f::count_thread_search_occurrences(s(0),s(1))),
            "findThreadSearchOccurrences"=>json!(f::find_thread_search_occurrences(s(0),s(1))),
            "searchableMessageSegments"=>{
                let mut item=args[0].clone();
                item["type"]=json!(format!("{}_message",item["role"].as_str().unwrap()));
                let skills: Vec<t3_contracts::orchestration_v2::SearchThreadInputSkillsItem>=serde_json::from_value(args.get(2).cloned().unwrap_or(json!([]))).unwrap();
                json!(rendered_segments(&item,args[1].as_str(),&skills))
            },
            "searchablePlanSegments"=>json!(rendered_segments(&json!({"type":"proposed_plan","markdown":s(0)}),args[1].as_str(),&[])),
            "foldSearchQuery"=>json!(p::fold_query(s(0))),
            "pickedQuery"=>json!(p::picked_query(s(0))),
            "messageSearchTokens"=>{let(long,short,words)=message_tokens(s(0));json!({"long":long,"short":short,"words":words})},
            "matchMessage"=>policy.match_message(s(0),s(1)).map_or(Value::Null,|(quality,offset)|json!({"quality":quality,"offset":offset})),
            "messageSearch"=>policy.message_search(s(0),args[1]==true).map_or(Value::Null,|r|json!({"match":r.match_query,"substrings":r.substrings,"shortSpellings":r.short_spellings})),
            "scoreThreadSearch"=>matched(p::score_thread(s(0),&serde_json::from_value::<p::ThreadSubject>(args[1].clone()).unwrap(),args[2].as_f64().unwrap(),args[3]["typos"]==true,args[3]["picks"]!=false)),
            "scoreSearchTerms"=>{
                let mut m=matched(p::score_terms(s(0),&serde_json::from_value::<Vec<String>>(args[1].clone()).unwrap(),args[2]["typos"]==true));
                if let Some(m)=m.as_object_mut(){m.remove("wholeWords");}
                m
            },
            "findSearchTokenRanges"=>ranges(&p::find_search_token_ranges(s(0),s(1))),
            "recordThreadOpen"=>json!(p::record_thread_open(args[0].clone(),args[1].as_f64().unwrap(),args[2].as_str())),
            "threadRecentUse"=>json!(p::recent_use(&serde_json::from_value(args[0].clone()).unwrap())),
            "compactDynamicToolOutput"=>json!(t3_shared::tool_output::compact_dynamic_tool_output(&args[0]).unwrap()),
            "htmlRenderFromToolItem"=>json!(t3_shared::tool_output::html_render_from_tool_item(&args[0]).unwrap()),
            "mcpAppFromToolItem"=>json!(t3_shared::tool_output::mcp_app_from_tool_item(&args[0]).unwrap()),
            "embeds"=>json!(NativeThreadFindText.embeds_result(&serde_json::from_value(args[0].clone()).unwrap())),
            "rankGroups"=>{
                let groups:Vec<_>=args[0].as_array().unwrap().iter().enumerate().map(|(i,g)|(i,(0..g["items"].as_array().unwrap().len()).collect::<Vec<_>>())).collect();
                let ranked=p::rank_groups(&groups,|item,typos,group|{
                    let value=args[1].as_array().unwrap().iter().find(|s|s[0]==*group&&s[1]==*item&&s[2]==typos).map(|s|s[3].clone()).unwrap_or(Value::Null);
                    if value.is_null(){return None;}
                    let mut m:p::SearchMatch=serde_json::from_value(value.clone()).unwrap();
                    m.whole_words=value["wholeWords"].as_bool().unwrap_or(m.quality>=1.0);
                    Some(m)
                });
                json!(ranked.iter().map(|g|json!({"group":args[0][*g.group]["group"],"indices":g.ranked.iter().map(|(i,_)|**i).collect::<Vec<_>>()})).collect::<Vec<_>>())
            },
            "rankMessages"=>json!(p::rank_message_matches(&serde_json::from_value::<Vec<p::MessageMatchCandidate>>(args[0].clone()).unwrap(),s(1),args[2].as_f64().unwrap(),args[3]["picks"]!=false,args[3]["withoutTitleMatches"]==true)),
            _=>panic!("unhandled {name}"),
        };
        if !equivalent(&actual, &case["expected"]) && failures.len() < 40 {
            failures.push(format!(
                "{name}: {}\nargs={}\nactual={actual}\nexpected={}",
                case["test"], args, case["expected"]
            ));
        }
    }
    eprintln!(
        "{} upstream tests; {} differential calls; {counts:?}",
        corpus["titles"].as_array().unwrap().len(),
        corpus["cases"].as_array().unwrap().len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn package09_terminal_corpus_is_equivalent() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/transport_terminal_output.json")).unwrap();
    for case in &cases {
        let mut state = t::TerminalOutputState::default();
        for chunk in case["chunks"].as_array().unwrap() {
            state = t::append_terminal_output(
                &state,
                chunk.as_str().unwrap(),
                case["max"].as_u64().unwrap() as usize,
            );
        }
        let actual = json!({"text":state.text,"pending":state.pending,"truncated":state.truncated,"resume":t::terminal_output_resume_text(&state),"plain":t::terminal_output_plain_text(&state.text)});
        for key in ["text", "pending", "truncated", "resume", "plain"] {
            assert_eq!(actual[key], case["expected"][key], "{} {key}", case["name"]);
        }
        let encoded = serde_json::to_value(&state).unwrap();
        for key in ["row", "col", "pen", "saved"] {
            assert_eq!(
                encoded["screen"][key], case["expected"][key],
                "{} {key}",
                case["name"]
            );
        }
        let units: usize = encoded["screen"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|l| l.as_array().unwrap())
            .map(|r| {
                r["text"].as_str().map_or_else(
                    || r["text"].as_array().unwrap().len(),
                    |s| s.encode_utf16().count(),
                )
            })
            .sum();
        assert_eq!(
            units,
            case["expected"]["windowUnits"].as_u64().unwrap() as usize,
            "{}",
            case["name"]
        );
    }
    eprintln!("{} package09 terminal cases", cases.len());
}
