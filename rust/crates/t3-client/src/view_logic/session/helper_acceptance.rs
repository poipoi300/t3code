#![allow(clippy::unwrap_used)]
use super::{links::*, review::*, terminal::*, text::*, *};
use crate::work_log::*;
use serde_json::{Value, json};
fn entry(value: &Value) -> WorkLogEntry {
    let mut value = value.clone();
    let object = value.as_object_mut().unwrap();
    for (key, value) in [
        ("id", json!("")),
        ("label", json!("")),
        ("createdAt", json!("")),
        ("tone", json!("info")),
        ("runId", Value::Null),
    ] {
        object.entry(key).or_insert(value);
    }
    serde_json::from_value(value).unwrap()
}
fn assert_case(case: &str) {
    let case: Value = serde_json::from_str(case).unwrap();
    let input = &case["input"];
    let s = |i: usize| input[i].as_str().unwrap_or_default();
    let n = |i: usize| input[i].as_u64().unwrap_or_default() as usize;
    let deserialize_item =
        || serde_json::from_value::<OrchestrationV2TurnItem>(input[0].clone()).unwrap();
    let actual = match case["function"].as_str().unwrap() {
        "derivePendingUserInputProgress" => {
            use crate::thread_views::questions::{Answer, DraftAnswer, Question, QuestionOption};
            let questions: Vec<_> = input[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|q| Question {
                    id: q["id"].as_str().unwrap().into(),
                    options: q["options"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|o| QuestionOption {
                            label: o["label"].as_str().unwrap().into(),
                            value: o["value"].as_str().map(str::to_string),
                        })
                        .collect(),
                    multi_select: q["multiSelect"] == true,
                    allow_custom_answer: q["allowCustomAnswer"] != false,
                    initial_answer: q
                        .get("initialAnswer")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect();
            let drafts: std::collections::BTreeMap<_, _> = input[1]
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, d)| {
                    (
                        key.clone(),
                        DraftAnswer {
                            selected_option_values: d
                                .get("selectedOptionValues")
                                .map(|v| serde_json::from_value(v.clone()).unwrap())
                                .unwrap_or_default(),
                            custom_answer: d["customAnswer"].as_str().map(str::to_string),
                            attachment_count: d["attachmentCount"].as_u64().unwrap_or(0) as usize,
                            attachments_blocked: d["attachmentsBlocked"] == true,
                        },
                    )
                })
                .collect();
            let progress = questions::pending_user_input_progress(
                &questions,
                &drafts,
                input[2].as_i64().unwrap(),
            );
            let answer = progress.resolved_answer.as_ref().map(|a| match a {
                Answer::Text(s) => json!(s),
                Answer::Choices(v) => json!(v),
            });
            let mut result = json!({"questionIndex":progress.question_index,"activeQuestion":input[0].get(progress.question_index),"selectedOptionValues":progress.selected_option_values,"customAnswer":progress.custom_answer,"resolvedAnswer":answer,"usingCustomAnswer":progress.using_custom_answer,"answeredQuestionCount":progress.answered_question_count,"isLastQuestion":progress.is_last_question,"isComplete":progress.is_complete,"canAdvance":progress.can_advance});
            if let Some(q) = progress.active_question
                && let Some(d) = input[1].get(&q.id)
            {
                result["activeDraft"] = d.clone();
            }
            result
        }
        "isLocalEditorFileHref" => json!(is_local_editor_file_href(s(0))),
        "rewriteMarkdownFileUriHref" => json!(rewrite_markdown_file_uri_href(s(0))),
        "resolveMarkdownFileLinkMeta" => json!(resolve_markdown_file_link_meta(
            s(0),
            input[1].as_str(),
            input[2].as_str().or(input[1].as_str())
        )),
        "resolveInlineCodeFileLinkMeta" => json!(resolve_inline_code_file_link_meta(
            s(0),
            input[1].as_str(),
            input[2].as_str().or(input[1].as_str())
        )),
        "isTerminalUrl" => json!(is_terminal_url(s(0))),
        "extractTerminalLinks" => json!(extract_terminal_links(s(0))),
        "resolveTerminalLinkTarget" => json!(resolve_terminal_link_target(s(0), s(1))),
        "textDirection" => json!(text_direction(s(0))),
        "markdownProseDirection" => json!(markdown_prose_direction(s(0))),
        "inferReviewCommentFenceLanguage" => json!(infer_review_comment_fence_language(s(0))),
        "formatReviewCommentFence" => json!(format_review_comment_fence(s(0), s(1))),
        "buildFileReviewComment" => {
            let i = &input[0];
            json!(build_file_review_comment(
                i["id"].as_str().unwrap(),
                i["filePath"].as_str().unwrap(),
                i["startLine"].as_i64().unwrap(),
                i["endLine"].as_i64().unwrap(),
                i["text"].as_str().unwrap(),
                i["contents"].as_str().unwrap()
            ))
        }
        "buildDiffReviewComment" => {
            let i = &input[0];
            json!(build_diff_review_comment(
                i["id"].as_str().unwrap(),
                i["sectionId"].as_str().unwrap(),
                i["sectionTitle"].as_str().unwrap(),
                i["filePath"].as_str().unwrap(),
                &serde_json::from_value(i["fileDiff"].clone()).unwrap(),
                &serde_json::from_value(i["range"].clone()).unwrap(),
                i["text"].as_str().unwrap()
            ))
        }
        "buildReviewCommentRenderablePatch" => json!(build_review_comment_renderable_patch(
            &serde_json::from_value(input[0].clone()).unwrap()
        )),
        "restoreDiffReviewCommentRange" => json!(restore_diff_review_comment_range(
            &serde_json::from_value(input[0].clone()).unwrap(),
            &serde_json::from_value(input[1].clone()).unwrap()
        )),
        "resolveDiffReviewPosition" => json!(resolve_diff_review_position(
            &serde_json::from_value(input[0].clone()).unwrap(),
            n(1),
            input[2].as_str().map(|s| if s == "deletions" {
                SelectionSide::Deletions
            } else {
                SelectionSide::Additions
            })
        )),
        "resolveDiffReviewAnchor" => json!(resolve_diff_review_anchor(
            &serde_json::from_value(input[0].clone()).unwrap(),
            &serde_json::from_value(input[1].clone()).unwrap(),
            input[2] == true
        )),
        "commandProgramName" => json!(command_program_name(s(0))),
        "commandDisplayText" => json!(command_display_text(s(0))),
        "withVisibleControlCharacters" => json!(with_visible_control_characters(s(0))),
        "embeddedScripts" => json!(embedded_scripts(s(0))),
        "isJsonToolText" => json!(is_json_tool_text(s(0))),
        "turnItemOutputText" => json!(turn_item_output_text(&deserialize_item())),
        "turnItemOutputImages" => json!(turn_item_output_images(&deserialize_item())),
        "toolItemForDisplay" => tool_item_for_display(&deserialize_item()),
        "workEntryIndicatesToolFailure" => {
            json!(work_entry_indicates_tool_failure(&entry(&input[0])))
        }
        "workEntryDisplayIndicatesToolFailure" => {
            json!(work_entry_display_indicates_tool_failure(&entry(&input[0])))
        }
        "resolveWorkEntryToolPresentation" => json!(resolve_work_entry_tool_presentation(
            &entry(&input[0]),
            input[1].as_str()
        )),
        "extractToolActivityPresentation" => json!(extract_tool_activity_presentation(&input[0])),
        "getQuestionAnswerText" => json!(question_answer_text(&input[0])),
        "getQuestionAnswerPreview" => json!(question_answer_preview(
            &serde_json::from_value(input[0].clone()).unwrap()
        )),
        "carryDisplacedCustomAnswerIntoPrompt" => json!(questions::carry_displaced_custom_answer(
            s(0),
            input[1].as_str()
        )),
        "withdrawDisplacedCustomAnswerFromPrompt" => {
            json!(questions::withdraw_displaced_custom_answer(
                s(0),
                s(1),
                &serde_json::from_value::<Vec<String>>(input.get(2).cloned().unwrap_or(json!([])))
                    .unwrap()
            ))
        }
        "threadContextsReferencedByAnswer" => {
            json!(
                questions::thread_contexts_referenced_by_answer(
                    s(0),
                    &serde_json::from_value::<
                        Vec<t3_contracts::composer_context::ThreadContextRecord>,
                    >(input[1].clone())
                    .unwrap()
                )
            )
        }
        "projectPendingUserInputAnswer" => {
            use t3_contracts::composer_context::{ReviewCommentContextRecord, ThreadContextRecord};
            let threads: Vec<ThreadContextRecord> = input[1]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["kind"] == "thread")
                .map(|r| serde_json::from_value(r.clone()).unwrap())
                .collect();
            let reviews: Vec<ReviewCommentContextRecord> = input[1]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["kind"] == "review-comment")
                .map(|r| serde_json::from_value(r.clone()).unwrap())
                .collect();
            let records: Vec<_> = threads
                .iter()
                .map(questions::AnswerContextRecord::Thread)
                .chain(reviews.iter().map(questions::AnswerContextRecord::Review))
                .collect();
            questions::project_pending_user_input_answer(&input[0], &records)
        }
        "isLatestRunSettled" => json!(is_latest_run_settled(
            serde_json::from_value::<Option<RunView>>(input[0].clone())
                .unwrap()
                .as_ref(),
            serde_json::from_value::<Option<RuntimeView>>(input[1].clone())
                .unwrap()
                .as_ref()
        )),
        "derivePhase" => json!(derive_phase(
            serde_json::from_value::<Option<RuntimeView>>(input[0].clone())
                .unwrap()
                .as_ref()
        )),
        "deriveCanInterruptRunningThread" => json!(derive_can_interrupt_running_thread(
            input[0] == true,
            serde_json::from_value::<Option<RuntimeView>>(input[1].clone())
                .unwrap()
                .as_ref()
        )),
        "providerErrorPresentation" => json!(provider_error_presentation(
            &serde_json::from_value(input[0].clone()).unwrap()
        )),
        "deriveRevertTurnCountByUserMessageId" => {
            let i = &input[0];
            let available: Option<std::collections::BTreeSet<u64>> = i
                .get("availableRevertTurnCounts")
                .map(|v| serde_json::from_value(v.clone()).unwrap());
            let unsettled: Option<IdOrIndex<RunId>> = i
                .get("unsettledRunId")
                .and_then(|v| serde_json::from_value(v.clone()).unwrap());
            json!(derive_revert_turn_count_by_user_message_id(
                &serde_json::from_value::<Vec<Rc<TimelineEntry>>>(i["timelineEntries"].clone())
                    .unwrap(),
                &serde_json::from_value::<Vec<CheckpointSummary>>(i["checkpoints"].clone())
                    .unwrap(),
                i["threadStartCheckpointAvailable"] == true,
                available.as_ref(),
                unsettled.as_ref()
            ))
        }
        "workLogEntryIsToolLike" => json!(work_log_entry_is_tool_like(&entry(&input[0]))),
        "workEntryIndicatesToolSuccess" => {
            json!(work_entry_indicates_tool_success(&entry(&input[0])))
        }
        "isStreamingTurnItemTextUpdate" => json!(is_streaming_turn_item_text_update(
            &serde_json::from_value(input[0].clone()).unwrap(),
            &serde_json::from_value(input[1].clone()).unwrap()
        )),
        "turnItemDetailRevision" => json!(turn_item_detail_revision(&deserialize_item())),
        "toolGroupAction" => json!(tool_group_action(&entry(&input[0]))),
        "workEntryViewedImagePath" => json!(work_entry_viewed_image_path(&entry(&input[0]))),
        "workEntriesViewedImagePaths" => json!(work_entries_viewed_image_paths(
            &input[0]
                .as_array()
                .unwrap()
                .iter()
                .map(entry)
                .collect::<Vec<_>>()
        )),
        "summarizeT3ToolCalls" => {
            let calls = input[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| T3ToolSummaryCall {
                    input: c["input"].clone(),
                    output: c["output"].clone(),
                    outcome: c["outcome"].as_str().unwrap().into(),
                })
                .collect::<Vec<_>>();
            json!(summarize_t3_tool_calls(s(0), &calls))
        }
        other => panic!("unhandled {other}"),
    };
    assert_eq!(
        actual, case["expected"],
        "{} input {}",
        case["function"], input
    );
}

// Captured TypeScript cases.
#[test]
fn ts_command_program_name_0() {
    assert_case(
        r####"{"function":"commandProgramName","input":["/bin/bash --rcfile /tmp/config -c 'git status'"],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_1() {
    assert_case(
        r####"{"function":"commandProgramName","input":["zsh -lc 'CI=1 env -u DEBUG sudo -u root vp test run'"],"expected":"vp"}"####,
    );
}
#[test]
fn ts_command_program_name_2() {
    assert_case(
        r####"{"function":"commandProgramName","input":["\"C:\\Program Files\\Git\\bin\\bash.exe\" -lc \"git status\""],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_3() {
    assert_case(
        r####"{"function":"commandProgramName","input":["/bin/zsh -lc 'git status\nsed -n '\"'1,20p' apps/web/src/components/DiffPanel.tsx\""],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_4() {
    assert_case(
        r####"{"function":"commandProgramName","input":["sudo -u root pnpm test"],"expected":"pnpm"}"####,
    );
}
#[test]
fn ts_command_program_name_5() {
    assert_case(
        r####"{"function":"commandProgramName","input":["zsh script.sh -c 'git status'"],"expected":"zsh"}"####,
    );
}
#[test]
fn ts_command_program_name_6() {
    assert_case(
        r####"{"function":"commandProgramName","input":["my-shell -c 'git status'"],"expected":"my-shell"}"####,
    );
}
#[test]
fn ts_command_program_name_7() {
    assert_case(
        r####"{"function":"commandProgramName","input":["$HOME/.bun/bin/bun test"],"expected":"bun"}"####,
    );
}
#[test]
fn ts_command_program_name_8() {
    assert_case(r####"{"function":"commandProgramName","input":["zsh -lc"],"expected":null}"####);
}
#[test]
fn ts_command_program_name_9() {
    assert_case(
        r####"{"function":"commandProgramName","input":["zsh -lc 'git status"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_10() {
    assert_case(
        r####"{"function":"commandProgramName","input":["zsh -lc 'env'"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_11() {
    assert_case(
        r####"{"function":"commandProgramName","input":["if test -f package.json; then vp test; fi"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_12() {
    assert_case(
        r####"{"function":"commandProgramName","input":["[[ -f package.json ]]"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_13() {
    assert_case(
        r####"{"function":"commandProgramName","input":["test -f package.json"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_14() {
    assert_case(
        r####"{"function":"commandProgramName","input":["for file in *; do echo \"$file\"; done"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_15() {
    assert_case(
        r####"{"function":"commandProgramName","input":["check() { vp test; }"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_16() {
    assert_case(
        r####"{"function":"commandProgramName","input":["{ vp test; }"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_17() {
    assert_case(
        r####"{"function":"commandProgramName","input":["(( count += 1 ))"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_18() {
    assert_case(
        r####"{"function":"commandProgramName","input":[". ./script.sh"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_19() {
    assert_case(
        r####"{"function":"commandProgramName","input":["export NODE_ENV=test"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_20() {
    assert_case(
        r####"{"function":"commandProgramName","input":["repeat 3 echo ok"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_21() {
    assert_case(r####"{"function":"commandProgramName","input":["return 1"],"expected":null}"####);
}
#[test]
fn ts_command_program_name_22() {
    assert_case(r####"{"function":"commandProgramName","input":["true"],"expected":null}"####);
}
#[test]
fn ts_command_program_name_23() {
    assert_case(
        r####"{"function":"commandProgramName","input":["rg -n \"if|for|while\" src"],"expected":"rg"}"####,
    );
}
#[test]
fn ts_command_program_name_24() {
    assert_case(
        r####"{"function":"commandProgramName","input":["vp test && git status"],"expected":"vp"}"####,
    );
}
#[test]
fn ts_command_program_name_25() {
    assert_case(
        r####"{"function":"commandProgramName","input":["echo `git status`"],"expected":"echo"}"####,
    );
}
#[test]
fn ts_command_program_name_26() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cat <(rg needle src)"],"expected":"cat"}"####,
    );
}
#[test]
fn ts_command_program_name_27() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd packages/client-runtime && vp test run"],"expected":"vp"}"####,
    );
}
#[test]
fn ts_command_program_name_28() {
    assert_case(
        r####"{"function":"commandProgramName","input":["PATH+=:/tools npm test"],"expected":"npm"}"####,
    );
}
#[test]
fn ts_command_program_name_29() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd $(find . -type d | head -1) && git status"],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_30() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp && # use the selected workspace\nnpm test"],"expected":"npm"}"####,
    );
}
#[test]
fn ts_command_program_name_31() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd ${ROOT:-path;with;semicolons} && bun test"],"expected":"bun"}"####,
    );
}
#[test]
fn ts_command_program_name_32() {
    assert_case(
        r####"{"function":"commandProgramName","input":["source ~/.nvm/nvm.sh && nvm use"],"expected":"nvm"}"####,
    );
}
#[test]
fn ts_command_program_name_33() {
    assert_case(
        r####"{"function":"commandProgramName","input":["unset DEBUG; node app.js"],"expected":"node"}"####,
    );
}
#[test]
fn ts_command_program_name_34() {
    assert_case(
        r####"{"function":"commandProgramName","input":["/bin/zsh -lc 'source ~/.nvm/nvm.sh && nvm use'"],"expected":"nvm"}"####,
    );
}
#[test]
fn ts_command_program_name_35() {
    assert_case(
        r####"{"function":"commandProgramName","input":["timeout 10s python3 script.py"],"expected":"python3"}"####,
    );
}
#[test]
fn ts_command_program_name_36() {
    assert_case(
        r####"{"function":"commandProgramName","input":["arch -arch arm64 /opt/tools/check"],"expected":"check"}"####,
    );
}
#[test]
fn ts_command_program_name_37() {
    assert_case(
        r####"{"function":"commandProgramName","input":["timeout 60 script -q /dev/null env CI=1 node app.js"],"expected":"node"}"####,
    );
}
#[test]
fn ts_command_program_name_38() {
    assert_case(
        r####"{"function":"commandProgramName","input":["command -v git"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_39() {
    assert_case(
        r####"{"function":"commandProgramName","input":["exec > output.log"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_40() {
    assert_case(
        r####"{"function":"commandProgramName","input":["env CI=1 cd /tmp && npm test"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_41() {
    assert_case(
        r####"{"function":"commandProgramName","input":["$PY scripts/check.py"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_42() {
    assert_case(
        r####"{"function":"commandProgramName","input":["& { Get-Process }"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_43() {
    assert_case(
        r####"{"function":"commandProgramName","input":["coproc npm test"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_44() {
    assert_case(
        r####"{"function":"commandProgramName","input":["hash --help"],"expected":"hash"}"####,
    );
}
#[test]
fn ts_command_program_name_45() {
    assert_case(
        r####"{"function":"commandProgramName","input":["Exec node app.js"],"expected":"Exec"}"####,
    );
}
#[test]
fn ts_command_program_name_46() {
    assert_case(
        r####"{"function":"commandProgramName","input":["CD /tmp && npm test"],"expected":"CD"}"####,
    );
}
#[test]
fn ts_command_program_name_47() {
    assert_case(
        r####"{"function":"commandProgramName","input":["# first comment\n  # second comment\ngit status"],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_48() {
    assert_case(
        r####"{"function":"commandProgramName","input":[">/tmp/log && npm test"],"expected":"npm"}"####,
    );
}
#[test]
fn ts_command_program_name_49() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp && 2>& 1 node app.js"],"expected":"node"}"####,
    );
}
#[test]
fn ts_command_program_name_50() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp && {output}>/tmp/log deno test"],"expected":"deno"}"####,
    );
}
#[test]
fn ts_command_program_name_51() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp <<-EOF\n\tnot-a-command\n\tEOF\nbun test"],"expected":"bun"}"####,
    );
}
#[test]
fn ts_command_program_name_52() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp <<A <<B\none\nA\ntwo\nB\ngit status"],"expected":"git"}"####,
    );
}
#[test]
fn ts_command_program_name_53() {
    assert_case(
        r####"{"function":"commandProgramName","input":["cd /tmp <<EOF && pnpm test\nnot-a-command\nEOF\nbun test"],"expected":"pnpm"}"####,
    );
}
#[test]
fn ts_command_program_name_54() {
    assert_case(
        r####"{"function":"commandProgramName","input":["sh -c 'sh -c '\\''sh -c '\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''sh -c '\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''git status'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\''\\'\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\''\\'\\'''\\'''\\'''"],"expected":null}"####,
    );
}
#[test]
fn ts_command_program_name_55() {
    assert_case(
        r####"{"function":"commandProgramName","input":["export VALUE_0=configured;export VALUE_1=configured;export VALUE_2=configured;export VALUE_3=configured;export VALUE_4=configured;export VALUE_5=configured;export VALUE_6=configured;export VALUE_7=configured;export VALUE_8=configured;export VALUE_9=configured;export VALUE_10=configured;export VALUE_11=configured;export VALUE_12=configured;export VALUE_13=configured;export VALUE_14=configured;export VALUE_15=configured;export VALUE_16=configured;export VALUE_17=configured;export VALUE_18=configured;export VALUE_19=configured;export VALUE_20=configured;export VALUE_21=configured;export VALUE_22=configured;export VALUE_23=configured;export VALUE_24=configured;export VALUE_25=configured;export VALUE_26=configured;export VALUE_27=configured;export VALUE_28=configured;export VALUE_29=configured;export VALUE_30=configured;export VALUE_31=configured;export VALUE_32=configured;export VALUE_33=configured;export VALUE_34=configured;export VALUE_35=configured;export VALUE_36=configured;export VALUE_37=configured;export VALUE_38=configured;export VALUE_39=configured;export VALUE_40=configured;export VALUE_41=configured;export VALUE_42=configured;export VALUE_43=configured;export VALUE_44=configured;export VALUE_45=configured;export VALUE_46=configured;export VALUE_47=configured;export VALUE_48=configured;export VALUE_49=configured;export VALUE_50=configured;export VALUE_51=configured;export VALUE_52=configured;export VALUE_53=configured;export VALUE_54=configured;export VALUE_55=configured;export VALUE_56=configured;export VALUE_57=configured;export VALUE_58=configured;export VALUE_59=configured;export VALUE_60=configured;export VALUE_61=configured;export VALUE_62=configured;export VALUE_63=configured;export VALUE_64=configured;export VALUE_65=configured;export VALUE_66=configured;export VALUE_67=configured;export VALUE_68=configured;export VALUE_69=configured;export VALUE_70=configured;export VALUE_71=configured;export VALUE_72=configured;export VALUE_73=configured;export VALUE_74=configured;export VALUE_75=configured;export VALUE_76=configured;export VALUE_77=configured;export VALUE_78=configured;export VALUE_79=configured;export VALUE_80=configured;export VALUE_81=configured;export VALUE_82=configured;export VALUE_83=configured;export VALUE_84=configured;export VALUE_85=configured;export VALUE_86=configured;export VALUE_87=configured;export VALUE_88=configured;export VALUE_89=configured;export VALUE_90=configured;export VALUE_91=configured;export VALUE_92=configured;export VALUE_93=configured;export VALUE_94=configured;export VALUE_95=configured;export VALUE_96=configured;export VALUE_97=configured;export VALUE_98=configured;export VALUE_99=configured;export VALUE_100=configured;export VALUE_101=configured;export VALUE_102=configured;export VALUE_103=configured;export VALUE_104=configured;export VALUE_105=configured;export VALUE_106=configured;export VALUE_107=configured;export VALUE_108=configured;export VALUE_109=configured;export VALUE_110=configured;export VALUE_111=configured;export VALUE_112=configured;export VALUE_113=configured;export VALUE_114=configured;export VALUE_115=configured;export VALUE_116=configured;export VALUE_117=configured;export VALUE_118=configured;export VALUE_119=configured;export VALUE_120=configured;export VALUE_121=configured;export VALUE_122=configured;export VALUE_123=configured;export VALUE_124=configured;export VALUE_125=configured;export VALUE_126=configured;export VALUE_127=configured;export VALUE_128=configured;export VALUE_129=configured;export VALUE_130=configured;export VALUE_131=configured;export VALUE_132=configured;export VALUE_133=configured;export VALUE_134=configured;export VALUE_135=configured;export VALUE_136=configured;export VALUE_137=configured;export VALUE_138=configured;export VALUE_139=configured;export VALUE_140=configured;export VALUE_141=configured;export VALUE_142=configured;export VALUE_143=configured;export VALUE_144=configured;export VALUE_145=configured;export VALUE_146=configured;export VALUE_147=configured;export VALUE_148=configured;export VALUE_149=configured;export VALUE_150=configured;export VALUE_151=configured;export VALUE_152=configured;export VALUE_153=configured;export VALUE_154=configured;export VALUE_155=configured;export VALUE_156=configured;export VALUE_157=configured;export VALUE_158=configured;export VALUE_159=configured;export VALUE_160=configured;export VALUE_161=configured;export VALUE_162=configured;export VALUE_163=configured;export VALUE_164=configured;export VALUE_165=configured;export VALUE_166=configured;export VALUE_167=configured;export VALUE_168=configured;export VALUE_169=configured;export VALUE_170=configured;export VALUE_171=configured;export VALUE_172=configured;export VALUE_173=configured;export VALUE_174=configured;export VALUE_175=configured;export VALUE_176=configured;export VALUE_177=configured;export VALUE_178=configured;export VALUE_179=configured;export VALUE_180=configured;export VALUE_181=configured;export VALUE_182=configured;export VALUE_183=configured;export VALUE_184=configured;export VALUE_185=configured;export VALUE_186=configured;export VALUE_187=configured;export VALUE_188=configured;export VALUE_189=configured;export VALUE_190=configured;export VALUE_191=configured;export VALUE_192=configured;export VALUE_193=configured;export VALUE_194=configured;export VALUE_195=configured;export VALUE_196=configured;export VALUE_197=configured;export VALUE_198=configured;export VALUE_199=configured;export VALUE_200=configured;export VALUE_201=configured;export VALUE_202=configured;export VALUE_203=configured;export VALUE_204=configured;export VALUE_205=configured;export VALUE_206=configured;export VALUE_207=configured;export VALUE_208=configured;export VALUE_209=configured;export VALUE_210=configured;export VALUE_211=configured;export VALUE_212=configured;export VALUE_213=configured;export VALUE_214=configured;export VALUE_215=configured;export VALUE_216=configured;export VALUE_217=configured;export VALUE_218=configured;export VALUE_219=configured;export VALUE_220=configured;export VALUE_221=configured;export VALUE_222=configured;export VALUE_223=configured;export VALUE_224=configured;export VALUE_225=configured;export VALUE_226=configured;export VALUE_227=configured;export VALUE_228=configured;export VALUE_229=configured;export VALUE_230=configured;export VALUE_231=configured;export VALUE_232=configured;export VALUE_233=configured;export VALUE_234=configured;export VALUE_235=configured;export VALUE_236=configured;export VALUE_237=configured;export VALUE_238=configured;export VALUE_239=configured;export VALUE_240=configured;export VALUE_241=configured;export VALUE_242=configured;export VALUE_243=configured;export VALUE_244=configured;export VALUE_245=configured;export VALUE_246=configured;export VALUE_247=configured;export VALUE_248=configured;export VALUE_249=configured;export VALUE_250=configured;export VALUE_251=configured;export VALUE_252=configured;export VALUE_253=configured;export VALUE_254=configured;export VALUE_255=configured;export VALUE_256=configured;export VALUE_257=configured;export VALUE_258=configured;export VALUE_259=configured;export VALUE_260=configured;export VALUE_261=configured;export VALUE_262=configured;export VALUE_263=configured;export VALUE_264=configured;export VALUE_265=configured;export VALUE_266=configured;export VALUE_267=configured;export VALUE_268=configured;export VALUE_269=configured;export VALUE_270=configured;export VALUE_271=configured;export VALUE_272=configured;export VALUE_273=configured;export VALUE_274=configured;export VALUE_275=configured;export VALUE_276=configured;export VALUE_277=configured;export VALUE_278=configured;export VALUE_279=configured;export VALUE_280=configured;export VALUE_281=configured;export VALUE_282=configured;export VALUE_283=configured;export VALUE_284=configured;export VALUE_285=configured;export VALUE_286=configured;export VALUE_287=configured;export VALUE_288=configured;export VALUE_289=configured;export VALUE_290=configured;export VALUE_291=configured;export VALUE_292=configured;export VALUE_293=configured;export VALUE_294=configured;export VALUE_295=configured;export VALUE_296=configured;export VALUE_297=configured;export VALUE_298=configured;export VALUE_299=configured;export VALUE_300=configured;export VALUE_301=configured;export VALUE_302=configured;export VALUE_303=configured;export VALUE_304=configured;export VALUE_305=configured;export VALUE_306=configured;export VALUE_307=configured;export VALUE_308=configured;export VALUE_309=configured;export VALUE_310=configured;export VALUE_311=configured;export VALUE_312=configured;export VALUE_313=configured;export VALUE_314=configured;export VALUE_315=configured;export VALUE_316=configured;export VALUE_317=configured;export VALUE_318=configured;export VALUE_319=configured;export VALUE_320=configured;export VALUE_321=configured;export VALUE_322=configured;export VALUE_323=configured;export VALUE_324=configured;export VALUE_325=configured;export VALUE_326=configured;export VALUE_327=configured;export VALUE_328=configured;export VALUE_329=configured;export VALUE_330=configured;export VALUE_331=configured;export VALUE_332=configured;export VALUE_333=configured;export VALUE_334=configured;export VALUE_335=configured;export VALUE_336=configured;export VALUE_337=configured;export VALUE_338=configured;export VALUE_339=configured;export VALUE_340=configured;export VALUE_341=configured;export VALUE_342=configured;export VALUE_343=configured;export VALUE_344=configured;export VALUE_345=configured;export VALUE_346=configured;export VALUE_347=configured;export VALUE_348=configured;export VALUE_349=configured;export VALUE_350=configured;export VALUE_351=configured;export VALUE_352=configured;export VALUE_353=configured;export VALUE_354=configured;export VALUE_355=configured;export VALUE_356=configured;export VALUE_357=configured;export VALUE_358=configured;export VALUE_359=configured;export VALUE_360=configured;export VALUE_361=configured;export VALUE_362=configured;export VALUE_363=configured;export VALUE_364=configured;export VALUE_365=configured;export VALUE_366=configured;export VALUE_367=configured;export VALUE_368=configured;export VALUE_369=configured;export VALUE_370=configured;export VALUE_371=configured;export VALUE_372=configured;export VALUE_373=configured;export VALUE_374=configured;export VALUE_375=configured;export VALUE_376=configured;export VALUE_377=configured;export VALUE_378=configured;export VALUE_379=configured;export VALUE_380=configured;export VALUE_381=configured;export VALUE_382=configured;export VALUE_383=configured;export VALUE_384=configured;export VALUE_385=configured;export VALUE_386=configured;export VALUE_387=configured;export VALUE_388=configured;export VALUE_389=configured;export VALUE_390=configured;export VALUE_391=configured;export VALUE_392=configured;export VALUE_393=configured;export VALUE_394=configured;export VALUE_395=configured;export VALUE_396=configured;export VALUE_397=configured;export VALUE_398=configured;export VALUE_399=configured;export VALUE_400=configured;export VALUE_401=configured;export VALUE_402=configured;export VALUE_403=configured;export VALUE_404=configured;export VALUE_405=configured;export VALUE_406=configured;export VALUE_407=configured;export VALUE_408=configured;export VALUE_409=configured;export VALUE_410=configured;export VALUE_411=configured;export VALUE_412=configured;export VALUE_413=configured;export VALUE_414=configured;export VALUE_415=configured;export VALUE_416=configured;export VALUE_417=configured;export VALUE_418=configured;export VALUE_419=configured;export VALUE_420=configured;export VALUE_421=configured;export VALUE_422=configured;export VALUE_423=configured;export VALUE_424=configured;export VALUE_425=configured;export VALUE_426=configured;export VALUE_427=configured;export VALUE_428=configured;export VALUE_429=configured;export VALUE_430=configured;export VALUE_431=configured;export VALUE_432=configured;export VALUE_433=configured;export VALUE_434=configured;export VALUE_435=configured;export VALUE_436=configured;export VALUE_437=configured;export VALUE_438=configured;export VALUE_439=configured;export VALUE_440=configured;export VALUE_441=configured;export VALUE_442=configured;export VALUE_443=configured;export VALUE_444=configured;export VALUE_445=configured;export VALUE_446=configured;export VALUE_447=configured;export VALUE_448=configured;export VALUE_449=configured;export VALUE_450=configured;export VALUE_451=configured;export VALUE_452=configured;export VALUE_453=configured;export VALUE_454=configured;export VALUE_455=configured;export VALUE_456=configured;export VALUE_457=configured;export VALUE_458=configured;export VALUE_459=configured;export VALUE_460=configured;export VALUE_461=configured;export VALUE_462=configured;export VALUE_463=configured;export VALUE_464=configured;export VALUE_465=configured;export VALUE_466=configured;export VALUE_467=configured;export VALUE_468=configured;export VALUE_469=configured;export VALUE_470=configured;export VALUE_471=configured;export VALUE_472=configured;export VALUE_473=configured;export VALUE_474=configured;export VALUE_475=configured;export VALUE_476=configured;export VALUE_477=configured;export VALUE_478=configured;export VALUE_479=configured;export VALUE_480=configured;export VALUE_481=configured;export VALUE_482=configured;export VALUE_483=configured;export VALUE_484=configured;export VALUE_485=configured;export VALUE_486=configured;export VALUE_487=configured;export VALUE_488=configured;export VALUE_489=configured;export VALUE_490=configured;export VALUE_491=configured;export VALUE_492=configured;export VALUE_493=configured;export VALUE_494=configured;export VALUE_495=configured;export VALUE_496=configured;export VALUE_497=configured;export VALUE_498=configured;export VALUE_499=configured;export VALUE_500=configured;export VALUE_501=configured;export VALUE_502=configured;export VALUE_503=configured;export VALUE_504=configured;export VALUE_505=configured;export VALUE_506=configured;export VALUE_507=configured;export VALUE_508=configured;export VALUE_509=configured;export VALUE_510=configured;export VALUE_511=configured;export VALUE_512=configured;export VALUE_513=configured;export VALUE_514=configured;export VALUE_515=configured;export VALUE_516=configured;export VALUE_517=configured;export VALUE_518=configured;export VALUE_519=configured;export VALUE_520=configured;export VALUE_521=configured;export VALUE_522=configured;export VALUE_523=configured;export VALUE_524=configured;export VALUE_525=configured;export VALUE_526=configured;export VALUE_527=configured;export VALUE_528=configured;export VALUE_529=configured;export VALUE_530=configured;export VALUE_531=configured;export VALUE_532=configured;export VALUE_533=configured;export VALUE_534=configured;export VALUE_535=configured;export VALUE_536=configured;export VALUE_537=configured;export VALUE_538=configured;export VALUE_539=configured;export VALUE_540=configured;export VALUE_541=configured;export VALUE_542=configured;export VALUE_543=configured;export VALUE_544=configured;export VALUE_545=configured;export VALUE_546=configured;export VALUE_547=configured;export VALUE_548=configured;export VALUE_549=configured;export VALUE_550=configured;export VALUE_551=configured;export VALUE_552=configured;export VALUE_553=configured;export VALUE_554=configured;export VALUE_555=configured;export VALUE_556=configured;export VALUE_557=configured;export VALUE_558=configured;export VALUE_559=configured;export VALUE_560=configured;export VALUE_561=configured;export VALUE_562=configured;export VALUE_563=configured;export VALUE_564=configured;export VALUE_565=configured;export VALUE_566=configured;export VALUE_567=configured;export VALUE_568=configured;export VALUE_569=configured;export VALUE_570=configured;export VALUE_571=configured;export VALUE_572=configured;export VALUE_573=configured;export VALUE_574=configured;export VALUE_575=configured;export VALUE_576=configured;export VALUE_577=configured;export VALUE_578=configured;export VALUE_579=configured;export VALUE_580=configured;export VALUE_581=configured;export VALUE_582=configured;export VALUE_583=configured;export VALUE_584=configured;export VALUE_585=configured;export VALUE_586=configured;export VALUE_587=configured;export VALUE_588=configured;export VALUE_589=configured;export VALUE_590=configured;export VALUE_591=configured;export VALUE_592=configured;export VALUE_593=configured;export VALUE_594=configured;export VALUE_595=configured;export VALUE_596=configured;export VALUE_597=configured;export VALUE_598=configured;export VALUE_599=configured;export VALUE_600=configured;export VALUE_601=configured;export VALUE_602=configured;export VALUE_603=configured;export VALUE_604=configured;export VALUE_605=configured;export VALUE_606=configured;export VALUE_607=configured;export VALUE_608=configured;export VALUE_609=configured;export VALUE_610=configured;export VALUE_611=configured;export VALUE_612=configured;export VALUE_613=configured;export VALUE_614=configured;export VALUE_615=configured;export VALUE_616=configured;export VALUE_617=configured;export VALUE_618=configured;export VALUE_619=configured;export VALUE_620=configured;export VALUE_621=configured;export VALUE_622=configured;export VALUE_623=configured;export VALUE_624=configured;export VALUE_625=configured;export VALUE_626=configured;export VALUE_627=configured;export VALUE_628=configured;export VALUE_629=configured;export VALUE_630=configured;export VALUE_631=configured;export VALUE_632=configured;export VALUE_633=configured;export VALUE_634=configured;export VALUE_635=configured;export VALUE_636=configured;export VALUE_637=configured;export VALUE_638=configured;export VALUE_639=configured;export VALUE_640=configured;export VALUE_641=configured;export VALUE_642=configured;export VALUE_643=configured;export VALUE_644=configured;export VALUE_645=configured;export VALUE_646=configured;export VALUE_647=configured;export VALUE_648=configured;export VALUE_649=configured;export VALUE_650=configured;export VALUE_651=configured;export VALUE_652=configured;export VALUE_653=configured;export VALUE_654=configured;export VALUE_655=configured;export VALUE_656=configured;export VALUE_657=configured;export VALUE_658=configured;export VALUE_659=configured;export VALUE_660=configured;export VALUE_661=configured;export VALUE_662=configured;export VALUE_663=configured;export VALUE_664=configured;export VALUE_665=configured;export VALUE_666=configured;export VALUE_667=configured;export VALUE_668=configured;export VALUE_669=configured;export VALUE_670=configured;export VALUE_671=configured;export VALUE_672=configured;export VALUE_673=configured;export VALUE_674=configured;export VALUE_675=configured;export VALUE_676=configured;export VALUE_677=configured;export VALUE_678=configured;export VALUE_679=configured;export VALUE_680=configured;export VALUE_681=configured;export VALUE_682=configured;export VALUE_683=configured;export VALUE_684=configured;export VALUE_685=configured;export VALUE_686=configured;export VALUE_687=configured;export VALUE_688=configured;export VALUE_689=configured;export VALUE_690=configured;export VALUE_691=configured;export VALUE_692=configured;export VALUE_693=configured;export VALUE_694=configured;export VALUE_695=configured;export VALUE_696=configured;export VALUE_697=configured;export VALUE_698=configured;export VALUE_699=configured;export VALUE_700=configured;export VALUE_701=configured;export VALUE_702=configured;export VALUE_703=configured;export VALUE_704=configured;export VALUE_705=configured;export VALUE_706=configured;export VALUE_707=configured;export VALUE_708=configured;export VALUE_709=configured;export VALUE_710=configured;export VALUE_711=configured;export VALUE_712=configured;export VALUE_713=configured;export VALUE_714=configured;export VALUE_715=configured;export VALUE_716=configured;export VALUE_717=configured;export VALUE_718=configured;export VALUE_719=configured;export VALUE_720=configured;export VALUE_721=configured;export VALUE_722=configured;export VALUE_723=configured;export VALUE_724=configured;export VALUE_725=configured;export VALUE_726=configured;export VALUE_727=configured;export VALUE_728=configured;export VALUE_729=configured;export VALUE_730=configured;export VALUE_731=configured;export VALUE_732=configured;export VALUE_733=configured;export VALUE_734=configured;export VALUE_735=configured;export VALUE_736=configured;export VALUE_737=configured;export VALUE_738=configured;export VALUE_739=configured;export VALUE_740=configured;export VALUE_741=configured;export VALUE_742=configured;export VALUE_743=configured;export VALUE_744=configured;export VALUE_745=configured;export VALUE_746=configured;export VALUE_747=configured;export VALUE_748=configured;export VALUE_749=configured;export VALUE_750=configured;export VALUE_751=configured;export VALUE_752=configured;export VALUE_753=configured;export VALUE_754=configured;export VALUE_755=configured;export VALUE_756=configured;export VALUE_757=configured;export VALUE_758=configured;export VALUE_759=configured;export VALUE_760=configured;export VALUE_761=configured;export VALUE_762=configured;export VALUE_763=configured;export VALUE_764=configured;export VALUE_765=configured;export VALUE_766=configured;export VALUE_767=configured;export VALUE_768=configured;export VALUE_769=configured;export VALUE_770=configured;export VALUE_771=configured;export VALUE_772=configured;export VALUE_773=configured;export VALUE_774=configured;export VALUE_775=configured;export VALUE_776=configured;export VALUE_777=configured;export VALUE_778=configured;export VALUE_779=configured;export VALUE_780=configured;export VALUE_781=configured;export VALUE_782=configured;export VALUE_783=configured;export VALUE_784=configured;export VALUE_785=configured;export VALUE_786=configured;export VALUE_787=configured;export VALUE_788=configured;export VALUE_789=configured;export VALUE_790=configured;export VALUE_791=configured;export VALUE_792=configured;export VALUE_793=configured;export VALUE_794=configured;export VALUE_795=configured;export VALUE_796=configured;export VALUE_797=configured;export VALUE_798=configured;export VALUE_799=configured;export VALUE_800=configured;export VALUE_801=configured;export VALUE_802=configured;export VALUE_803=configured;export VALUE_804=configured;export VALUE_805=configured;export VALUE_806=configured;export VALUE_807=configured;export VALUE_808=configured;export VALUE_809=configured;export VALUE_810=configured;export VALUE_811=configured;export VALUE_812=configured;export VALUE_813=configured;export VALUE_814=configured;export VALUE_815=configured;export VALUE_816=configured;export VALUE_817=configured;export VALUE_818=configured;export VALUE_819=configured;export VALUE_820=configured;export VALUE_821=configured;export VALUE_822=configured;export VALUE_823=configured;export VALUE_824=configured;export VALUE_825=configured;export VALUE_826=configured;export VALUE_827=configured;export VALUE_828=configured;export VALUE_829=configured;export VALUE_830=configured;export VALUE_831=configured;export VALUE_832=configured;export VALUE_833=configured;export VALUE_834=configured;export VALUE_835=configured;export VALUE_836=configured;export VALUE_837=configured;export VALUE_838=configured;export VALUE_839=configured;export VALUE_840=configured;export VALUE_841=configured;export VALUE_842=configured;export VALUE_843=configured;export VALUE_844=configured;export VALUE_845=configured;export VALUE_846=configured;export VALUE_847=configured;export VALUE_848=configured;export VALUE_849=configured;export VALUE_850=configured;export VALUE_851=configured;export VALUE_852=configured;export VALUE_853=configured;export VALUE_854=configured;export VALUE_855=configured;export VALUE_856=configured;export VALUE_857=configured;export VALUE_858=configured;export VALUE_859=configured;export VALUE_860=configured;export VALUE_861=configured;export VALUE_862=configured;export VALUE_863=configured;export VALUE_864=configured;export VALUE_865=configured;export VALUE_866=configured;export VALUE_867=configured;export VALUE_868=configured;export VALUE_869=configured;export VALUE_870=configured;export VALUE_871=configured;export VALUE_872=configured;export VALUE_873=configured;export VALUE_874=configured;export VALUE_875=configured;export VALUE_876=configured;export VALUE_877=configured;export VALUE_878=configured;export VALUE_879=configured;export VALUE_880=configured;export VALUE_881=configured;export VALUE_882=configured;export VALUE_883=configured;export VALUE_884=configured;export VALUE_885=configured;export VALUE_886=configured;export VALUE_887=configured;export VALUE_888=configured;export VALUE_889=configured;export VALUE_890=configured;export VALUE_891=configured;export VALUE_892=configured;export VALUE_893=configured;export VALUE_894=configured;export VALUE_895=configured;export VALUE_896=configured;export VALUE_897=configured;export VALUE_898=configured;export VALUE_899=configured;export VALUE_900=configured;export VALUE_901=configured;export VALUE_902=configured;export VALUE_903=configured;export VALUE_904=configured;export VALUE_905=configured;export VALUE_906=configured;export VALUE_907=configured;export VALUE_908=configured;export VALUE_909=configured;export VALUE_910=configured;export VALUE_911=configured;export VALUE_912=configured;export VALUE_913=configured;export VALUE_914=configured;export VALUE_915=configured;export VALUE_916=configured;export VALUE_917=configured;export VALUE_918=configured;export VALUE_919=configured;export VALUE_920=configured;export VALUE_921=configured;export VALUE_922=configured;export VALUE_923=configured;export VALUE_924=configured;export VALUE_925=configured;export VALUE_926=configured;export VALUE_927=configured;export VALUE_928=configured;export VALUE_929=configured;export VALUE_930=configured;export VALUE_931=configured;export VALUE_932=configured;export VALUE_933=configured;export VALUE_934=configured;export VALUE_935=configured;export VALUE_936=configured;export VALUE_937=configured;export VALUE_938=configured;export VALUE_939=configured;export VALUE_940=configured;export VALUE_941=configured;export VALUE_942=configured;export VALUE_943=configured;export VALUE_944=configured;export VALUE_945=configured;export VALUE_946=configured;export VALUE_947=configured;export VALUE_948=configured;export VALUE_949=configured;export VALUE_950=configured;export VALUE_951=configured;export VALUE_952=configured;export VALUE_953=configured;export VALUE_954=configured;export VALUE_955=configured;export VALUE_956=configured;export VALUE_957=configured;export VALUE_958=configured;export VALUE_959=configured;export VALUE_960=configured;export VALUE_961=configured;export VALUE_962=configured;export VALUE_963=configured;export VALUE_964=configured;export VALUE_965=configured;export VALUE_966=configured;export VALUE_967=configured;export VALUE_968=configured;export VALUE_969=configured;export VALUE_970=configured;export VALUE_971=configured;export VALUE_972=configured;export VALUE_973=configured;export VALUE_974=configured;export VALUE_975=configured;export VALUE_976=configured;export VALUE_977=configured;export VALUE_978=configured;export VALUE_979=configured;export VALUE_980=configured;export VALUE_981=configured;export VALUE_982=configured;export VALUE_983=configured;export VALUE_984=configured;export VALUE_985=configured;export VALUE_986=configured;export VALUE_987=configured;export VALUE_988=configured;export VALUE_989=configured;export VALUE_990=configured;export VALUE_991=configured;export VALUE_992=configured;export VALUE_993=configured;export VALUE_994=configured;export VALUE_995=configured;export VALUE_996=configured;export VALUE_997=configured;export VALUE_998=configured;export VALUE_999=configured;export VALUE_1000=configured;export VALUE_1001=configured;export VALUE_1002=configured;export VALUE_1003=configured;export VALUE_1004=configured;export VALUE_1005=configured;export VALUE_1006=configured;export VALUE_1007=configured;export VALUE_1008=configured;export VALUE_1009=configured;export VALUE_1010=configured;export VALUE_1011=configured;export VALUE_1012=configured;export VALUE_1013=configured;export VALUE_1014=configured;export VALUE_1015=configured;export VALUE_1016=configured;export VALUE_1017=configured;export VALUE_1018=configured;export VALUE_1019=configured;export VALUE_1020=configured;export VALUE_1021=configured;export VALUE_1022=configured;export VALUE_1023=configured;export VALUE_1024=configured;export VALUE_1025=configured;export VALUE_1026=configured;export VALUE_1027=configured;export VALUE_1028=configured;export VALUE_1029=configured;export VALUE_1030=configured;export VALUE_1031=configured;export VALUE_1032=configured;export VALUE_1033=configured;export VALUE_1034=configured;export VALUE_1035=configured;export VALUE_1036=configured;export VALUE_1037=configured;export VALUE_1038=configured;export VALUE_1039=configured;export VALUE_1040=configured;export VALUE_1041=configured;export VALUE_1042=configured;export VALUE_1043=configured;export VALUE_1044=configured;export VALUE_1045=configured;export VALUE_1046=configured;export VALUE_1047=configured;export VALUE_1048=configured;export VALUE_1049=configured;export VALUE_1050=configured;export VALUE_1051=configured;export VALUE_1052=configured;export VALUE_1053=configured;export VALUE_1054=configured;export VALUE_1055=configured;export VALUE_1056=configured;export VALUE_1057=configured;export VALUE_1058=configured;export VALUE_1059=configured;export VALUE_1060=configured;export VALUE_1061=configured;export VALUE_1062=configured;export VALUE_1063=configured;export VALUE_1064=configured;export VALUE_1065=configured;export VALUE_1066=configured;export VALUE_1067=configured;export VALUE_1068=configured;export VALUE_1069=configured;export VALUE_1070=configured;export VALUE_1071=configured;export VALUE_1072=configured;export VALUE_1073=configured;export VALUE_1074=configured;export VALUE_1075=configured;export VALUE_1076=configured;export VALUE_1077=configured;export VALUE_1078=configured;export VALUE_1079=configured;export VALUE_1080=configured;export VALUE_1081=configured;export VALUE_1082=configured;export VALUE_1083=configured;export VALUE_1084=configured;export VALUE_1085=configured;export VALUE_1086=configured;export VALUE_1087=configured;export VALUE_1088=configured;export VALUE_1089=configured;export VALUE_1090=configured;export VALUE_1091=configured;export VALUE_1092=configured;export VALUE_1093=configured;export VALUE_1094=configured;export VALUE_1095=configured;export VALUE_1096=configured;export VALUE_1097=configured;export VALUE_1098=configured;export VALUE_1099=configured;export VALUE_1100=configured;export VALUE_1101=configured;export VALUE_1102=configured;export VALUE_1103=configured;export VALUE_1104=configured;export VALUE_1105=configured;export VALUE_1106=configured;export VALUE_1107=configured;export VALUE_1108=configured;export VALUE_1109=configured;export VALUE_1110=configured;export VALUE_1111=configured;export VALUE_1112=configured;export VALUE_1113=configured;export VALUE_1114=configured;export VALUE_1115=configured;export VALUE_1116=configured;export VALUE_1117=configured;export VALUE_1118=configured;export VALUE_1119=configured;export VALUE_1120=configured;export VALUE_1121=configured;export VALUE_1122=configured;export VALUE_1123=configured;export VALUE_1124=configured;export VALUE_1125=configured;export VALUE_1126=configured;export VALUE_1127=configured;export VALUE_1128=configured;export VALUE_1129=configured;export VALUE_1130=configured;export VALUE_1131=configured;export VALUE_1132=configured;export VALUE_1133=configured;export VALUE_1134=configured;export VALUE_1135=configured;export VALUE_1136=configured;export VALUE_1137=configured;export VALUE_1138=configured;export VALUE_1139=configured;export VALUE_1140=configured;export VALUE_1141=configured;export VALUE_1142=configured;export VALUE_1143=configured;export VALUE_1144=configured;export VALUE_1145=configured;export VALUE_1146=configured;export VALUE_1147=configured;export VALUE_1148=configured;export VALUE_1149=configured;export VALUE_1150=configured;export VALUE_1151=configured;export VALUE_1152=configured;export VALUE_1153=configured;export VALUE_1154=configured;export VALUE_1155=configured;export VALUE_1156=configured;export VALUE_1157=configured;export VALUE_1158=configured;export VALUE_1159=configured;export VALUE_1160=configured;export VALUE_1161=configured;export VALUE_1162=configured;export VALUE_1163=configured;export VALUE_1164=configured;export VALUE_1165=configured;export VALUE_1166=configured;export VALUE_1167=configured;export VALUE_1168=configured;export VALUE_1169=configured;export VALUE_1170=configured;export VALUE_1171=configured;export VALUE_1172=configured;export VALUE_1173=configured;export VALUE_1174=configured;export VALUE_1175=configured;export VALUE_1176=configured;export VALUE_1177=configured;export VALUE_1178=configured;export VALUE_1179=configured;export VALUE_1180=configured;export VALUE_1181=configured;export VALUE_1182=configured;export VALUE_1183=configured;export VALUE_1184=configured;export VALUE_1185=configured;export VALUE_1186=configured;export VALUE_1187=configured;export VALUE_1188=configured;export VALUE_1189=configured;export VALUE_1190=configured;export VALUE_1191=configured;export VALUE_1192=configured;export VALUE_1193=configured;export VALUE_1194=configured;export VALUE_1195=configured;export VALUE_1196=configured;export VALUE_1197=configured;export VALUE_1198=configured;export VALUE_1199=configured;export VALUE_1200=configured;export VALUE_1201=configured;export VALUE_1202=configured;export VALUE_1203=configured;export VALUE_1204=configured;export VALUE_1205=configured;export VALUE_1206=configured;export VALUE_1207=configured;export VALUE_1208=configured;export VALUE_1209=configured;export VALUE_1210=configured;export VALUE_1211=configured;export VALUE_1212=configured;export VALUE_1213=configured;export VALUE_1214=configured;export VALUE_1215=configured;export VALUE_1216=configured;export VALUE_1217=configured;export VALUE_1218=configured;export VALUE_1219=configured;export VALUE_1220=configured;export VALUE_1221=configured;export VALUE_1222=configured;export VALUE_1223=configured;export VALUE_1224=configured;export VALUE_1225=configured;export VALUE_1226=configured;export VALUE_1227=configured;export VALUE_1228=configured;export VALUE_1229=configured;export VALUE_1230=configured;export VALUE_1231=configured;export VALUE_1232=configured;export VALUE_1233=configured;export VALUE_1234=configured;export VALUE_1235=configured;export VALUE_1236=configured;export VALUE_1237=configured;export VALUE_1238=configured;export VALUE_1239=configured;export VALUE_1240=configured;export VALUE_1241=configured;export VALUE_1242=configured;export VALUE_1243=configured;export VALUE_1244=configured;export VALUE_1245=configured;export VALUE_1246=configured;export VALUE_1247=configured;export VALUE_1248=configured;export VALUE_1249=configured;export VALUE_1250=configured;export VALUE_1251=configured;export VALUE_1252=configured;export VALUE_1253=configured;export VALUE_1254=configured;export VALUE_1255=configured;export VALUE_1256=configured;export VALUE_1257=configured;export VALUE_1258=configured;export VALUE_1259=configured;export VALUE_1260=configured;export VALUE_1261=configured;export VALUE_1262=configured;export VALUE_1263=configured;export VALUE_1264=configured;export VALUE_1265=configured;export VALUE_1266=configured;export VALUE_1267=configured;export VALUE_1268=configured;export VALUE_1269=configured;export VALUE_1270=configured;export VALUE_1271=configured;export VALUE_1272=configured;export VALUE_1273=configured;export VALUE_1274=configured;export VALUE_1275=configured;export VALUE_1276=configured;export VALUE_1277=configured;export VALUE_1278=configured;export VALUE_1279=configured;export VALUE_1280=configured;export VALUE_1281=configured;export VALUE_1282=configured;export VALUE_1283=configured;export VALUE_1284=configured;export VALUE_1285=configured;export VALUE_1286=configured;export VALUE_1287=configured;export VALUE_1288=configured;export VALUE_1289=configured;export VALUE_1290=configured;export VALUE_1291=configured;export VALUE_1292=configured;export VALUE_1293=configured;export VALUE_1294=configured;export VALUE_1295=configured;export VALUE_1296=configured;export VALUE_1297=configured;export VALUE_1298=configured;export VALUE_1299=configured;export VALUE_1300=configured;export VALUE_1301=configured;export VALUE_1302=configured;export VALUE_1303=configured;export VALUE_1304=configured;export VALUE_1305=configured;export VALUE_1306=configured;export VALUE_1307=configured;export VALUE_1308=configured;export VALUE_1309=configured;export VALUE_1310=configured;export VALUE_1311=configured;export VALUE_1312=configured;export VALUE_1313=configured;export VALUE_1314=configured;export VALUE_1315=configured;export VALUE_1316=configured;export VALUE_1317=configured;export VALUE_1318=configured;export VALUE_1319=configured;export VALUE_1320=configured;export VALUE_1321=configured;export VALUE_1322=configured;export VALUE_1323=configured;export VALUE_1324=configured;export VALUE_1325=configured;export VALUE_1326=configured;export VALUE_1327=configured;export VALUE_1328=configured;export VALUE_1329=configured;export VALUE_1330=configured;export VALUE_1331=configured;export VALUE_1332=configured;export VALUE_1333=configured;export VALUE_1334=configured;export VALUE_1335=configured;export VALUE_1336=configured;export VALUE_1337=configured;export VALUE_1338=configured;export VALUE_1339=configured;export VALUE_1340=configured;export VALUE_1341=configured;export VALUE_1342=configured;export VALUE_1343=configured;export VALUE_1344=configured;export VALUE_1345=configured;export VALUE_1346=configured;export VALUE_1347=configured;export VALUE_1348=configured;export VALUE_1349=configured;export VALUE_1350=configured;export VALUE_1351=configured;export VALUE_1352=configured;export VALUE_1353=configured;export VALUE_1354=configured;export VALUE_1355=configured;export VALUE_1356=configured;export VALUE_1357=configured;export VALUE_1358=configured;export VALUE_1359=configured;export VALUE_1360=configured;export VALUE_1361=configured;export VALUE_1362=configured;export VALUE_1363=configured;export VALUE_1364=configured;export VALUE_1365=configured;export VALUE_1366=configured;export VALUE_1367=configured;export VALUE_1368=configured;export VALUE_1369=configured;export VALUE_1370=configured;export VALUE_1371=configured;export VALUE_1372=configured;export VALUE_1373=configured;export VALUE_1374=configured;export VALUE_1375=configured;export VALUE_1376=configured;export VALUE_1377=configured;export VALUE_1378=configured;export VALUE_1379=configured;export VALUE_1380=configured;export VALUE_1381=configured;export VALUE_1382=configured;export VALUE_1383=configured;export VALUE_1384=configured;export VALUE_1385=configured;export VALUE_1386=configured;export VALUE_1387=configured;export VALUE_1388=configured;export VALUE_1389=configured;export VALUE_1390=configured;export VALUE_1391=configured;export VALUE_1392=configured;export VALUE_1393=configured;export VALUE_1394=configured;export VALUE_1395=configured;export VALUE_1396=configured;export VALUE_1397=configured;export VALUE_1398=configured;export VALUE_1399=configured;export VALUE_1400=configured;export VALUE_1401=configured;export VALUE_1402=configured;export VALUE_1403=configured;export VALUE_1404=configured;export VALUE_1405=configured;export VALUE_1406=configured;export VALUE_1407=configured;export VALUE_1408=configured;export VALUE_1409=configured;export VALUE_1410=configured;export VALUE_1411=configured;export VALUE_1412=configured;export VALUE_1413=configured;export VALUE_1414=configured;export VALUE_1415=configured;export VALUE_1416=configured;export VALUE_1417=configured;export VALUE_1418=configured;export VALUE_1419=configured;export VALUE_1420=configured;export VALUE_1421=configured;export VALUE_1422=configured;export VALUE_1423=configured;export VALUE_1424=configured;export VALUE_1425=configured;export VALUE_1426=configured;export VALUE_1427=configured;export VALUE_1428=configured;export VALUE_1429=configured;export VALUE_1430=configured;export VALUE_1431=configured;export VALUE_1432=configured;export VALUE_1433=configured;export VALUE_1434=configured;export VALUE_1435=configured;export VALUE_1436=configured;export VALUE_1437=configured;export VALUE_1438=configured;export VALUE_1439=configured;export VALUE_1440=configured;export VALUE_1441=configured;export VALUE_1442=configured;export VALUE_1443=configured;export VALUE_1444=configured;export VALUE_1445=configured;export VALUE_1446=configured;export VALUE_1447=configured;export VALUE_1448=configured;export VALUE_1449=configured;export VALUE_1450=configured;export VALUE_1451=configured;export VALUE_1452=configured;export VALUE_1453=configured;export VALUE_1454=configured;export VALUE_1455=configured;export VALUE_1456=configured;export VALUE_1457=configured;export VALUE_1458=configured;export VALUE_1459=configured;export VALUE_1460=configured;export VALUE_1461=configured;export VALUE_1462=configured;export VALUE_1463=configured;export VALUE_1464=configured;export VALUE_1465=configured;export VALUE_1466=configured;export VALUE_1467=configured;export VALUE_1468=configured;export VALUE_1469=configured;export VALUE_1470=configured;export VALUE_1471=configured;export VALUE_1472=configured;export VALUE_1473=configured;export VALUE_1474=configured;export VALUE_1475=configured;export VALUE_1476=configured;export VALUE_1477=configured;export VALUE_1478=configured;export VALUE_1479=configured;export VALUE_1480=configured;export VALUE_1481=configured;export VALUE_1482=configured;export VALUE_1483=configured;export VALUE_1484=configured;export VALUE_1485=configured;export VALUE_1486=configured;export VALUE_1487=configured;export VALUE_1488=configured;export VALUE_1489=configured;export VALUE_1490=configured;export VALUE_1491=configured;export VALUE_1492=configured;export VALUE_1493=configured;export VALUE_1494=configured;export VALUE_1495=configured;export VALUE_1496=configured;export VALUE_1497=configured;export VALUE_1498=configured;export VALUE_1499=configured;export VALUE_1500=configured;export VALUE_1501=configured;export VALUE_1502=configured;export VALUE_1503=configured;export VALUE_1504=configured;export VALUE_1505=configured;export VALUE_1506=configured;export VALUE_1507=configured;export VALUE_1508=configured;export VALUE_1509=configured;export VALUE_1510=configured;export VALUE_1511=configured;export VALUE_1512=configured;export VALUE_1513=configured;export VALUE_1514=configured;export VALUE_1515=configured;export VALUE_1516=configured;export VALUE_1517=configured;export VALUE_1518=configured;export VALUE_1519=configured;export VALUE_1520=configured;export VALUE_1521=configured;export VALUE_1522=configured;export VALUE_1523=configured;export VALUE_1524=configured;export VALUE_1525=configured;export VALUE_1526=configured;export VALUE_1527=configured;export VALUE_1528=configured;export VALUE_1529=configured;export VALUE_1530=configured;export VALUE_1531=configured;export VALUE_1532=configured;export VALUE_1533=configured;export VALUE_1534=configured;export VALUE_1535=configured;export VALUE_1536=configured;export VALUE_1537=configured;export VALUE_1538=configured;export VALUE_1539=configured;export VALUE_1540=configured;export VALUE_1541=configured;export VALUE_1542=configured;export VALUE_1543=configured;export VALUE_1544=configured;export VALUE_1545=configured;export VALUE_1546=configured;export VALUE_1547=configured;export VALUE_1548=configured;export VALUE_1549=configured;export VALUE_1550=configured;export VALUE_1551=configured;export VALUE_1552=configured;export VALUE_1553=configured;export VALUE_1554=configured;export VALUE_1555=configured;export VALUE_1556=configured;export VALUE_1557=configured;export VALUE_1558=configured;export VALUE_1559=configured;export VALUE_1560=configured;export VALUE_1561=configured;export VALUE_1562=configured;export VALUE_1563=configured;export VALUE_1564=configured;export VALUE_1565=configured;export VALUE_1566=configured;export VALUE_1567=configured;export VALUE_1568=configured;export VALUE_1569=configured;export VALUE_1570=configured;export VALUE_1571=configured;export VALUE_1572=configured;export VALUE_1573=configured;export VALUE_1574=configured;export VALUE_1575=configured;export VALUE_1576=configured;export VALUE_1577=configured;export VALUE_1578=configured;export VALUE_1579=configured;export VALUE_1580=configured;export VALUE_1581=configured;export VALUE_1582=configured;export VALUE_1583=configured;export VALUE_1584=configured;export VALUE_1585=configured;export VALUE_1586=configured;export VALUE_1587=configured;export VALUE_1588=configured;export VALUE_1589=configured;export VALUE_1590=configured;export VALUE_1591=configured;export VALUE_1592=configured;export VALUE_1593=configured;export VALUE_1594=configured;export VALUE_1595=configured;export VALUE_1596=configured;export VALUE_1597=configured;export VALUE_1598=configured;export VALUE_1599=configured;export VALUE_1600=configured;export VALUE_1601=configured;export VALUE_1602=configured;export VALUE_1603=configured;export VALUE_1604=configured;export VALUE_1605=configured;export VALUE_1606=configured;export VALUE_1607=configured;export VALUE_1608=configured;export VALUE_1609=configured;export VALUE_1610=configured;export VALUE_1611=configured;export VALUE_1612=configured;export VALUE_1613=configured;export VALUE_1614=configured;export VALUE_1615=configured;export VALUE_1616=configured;export VALUE_1617=configured;export VALUE_1618=configured;export VALUE_1619=configured;export VALUE_1620=configured;export VALUE_1621=configured;export VALUE_1622=configured;export VALUE_1623=configured;export VALUE_1624=configured;export VALUE_1625=configured;export VALUE_1626=configured;export VALUE_1627=configured;export VALUE_1628=configured;export VALUE_1629=configured;export VALUE_1630=configured;export VALUE_1631=configured;export VALUE_1632=configured;export VALUE_1633=configured;export VALUE_1634=configured;export VALUE_1635=configured;export VALUE_1636=configured;export VALUE_1637=configured;export VALUE_1638=configured;export VALUE_1639=configured;export VALUE_1640=configured;export VALUE_1641=configured;export VALUE_1642=configured;export VALUE_1643=configured;export VALUE_1644=configured;export VALUE_1645=configured;export VALUE_1646=configured;export VALUE_1647=configured;export VALUE_1648=configured;export VALUE_1649=configured;export VALUE_1650=configured;export VALUE_1651=configured;export VALUE_1652=configured;export VALUE_1653=configured;export VALUE_1654=configured;export VALUE_1655=configured;export VALUE_1656=configured;export VALUE_1657=configured;export VALUE_1658=configured;export VALUE_1659=configured;export VALUE_1660=configured;export VALUE_1661=configured;export VALUE_1662=configured;export VALUE_1663=configured;export VALUE_1664=configured;export VALUE_1665=configured;export VALUE_1666=configured;export VALUE_1667=configured;export VALUE_1668=configured;export VALUE_1669=configured;export VALUE_1670=configured;export VALUE_1671=configured;export VALUE_1672=configured;export VALUE_1673=configured;export VALUE_1674=configured;export VALUE_1675=configured;export VALUE_1676=configured;export VALUE_1677=configured;export VALUE_1678=configured;export VALUE_1679=configured;export VALUE_1680=configured;export VALUE_1681=configured;export VALUE_1682=configured;export VALUE_1683=configured;export VALUE_1684=configured;export VALUE_1685=configured;export VALUE_1686=configured;export VALUE_1687=configured;export VALUE_1688=configured;export VALUE_1689=configured;export VALUE_1690=configured;export VALUE_1691=configured;export VALUE_1692=configured;export VALUE_1693=configured;export VALUE_1694=configured;export VALUE_1695=configured;export VALUE_1696=configured;export VALUE_1697=configured;export VALUE_1698=configured;export VALUE_1699=configured;export VALUE_1700=configured;export VALUE_1701=configured;export VALUE_1702=configured;export VALUE_1703=configured;export VALUE_1704=configured;export VALUE_1705=configured;export VALUE_1706=configured;export VALUE_1707=configured;export VALUE_1708=configured;export VALUE_1709=configured;export VALUE_1710=configured;export VALUE_1711=configured;export VALUE_1712=configured;export VALUE_1713=configured;export VALUE_1714=configured;export VALUE_1715=configured;export VALUE_1716=configured;export VALUE_1717=configured;export VALUE_1718=configured;export VALUE_1719=configured;export VALUE_1720=configured;export VALUE_1721=configured;export VALUE_1722=configured;export VALUE_1723=configured;export VALUE_1724=configured;export VALUE_1725=configured;export VALUE_1726=configured;export VALUE_1727=configured;export VALUE_1728=configured;export VALUE_1729=configured;export VALUE_1730=configured;export VALUE_1731=configured;export VALUE_1732=configured;export VALUE_1733=configured;export VALUE_1734=configured;export VALUE_1735=configured;export VALUE_1736=configured;export VALUE_1737=configured;export VALUE_1738=configured;export VALUE_1739=configured;export VALUE_1740=configured;export VALUE_1741=configured;export VALUE_1742=configured;export VALUE_1743=configured;export VALUE_1744=configured;export VALUE_1745=configured;export VALUE_1746=configured;export VALUE_1747=configured;export VALUE_1748=configured;export VALUE_1749=configured;export VALUE_1750=configured;export VALUE_1751=configured;export VALUE_1752=configured;export VALUE_1753=configured;export VALUE_1754=configured;export VALUE_1755=configured;export VALUE_1756=configured;export VALUE_1757=configured;export VALUE_1758=configured;export VALUE_1759=configured;export VALUE_1760=configured;export VALUE_1761=configured;export VALUE_1762=configured;export VALUE_1763=configured;export VALUE_1764=configured;export VALUE_1765=configured;export VALUE_1766=configured;export VALUE_1767=configured;export VALUE_1768=configured;export VALUE_1769=configured;export VALUE_1770=configured;export VALUE_1771=configured;export VALUE_1772=configured;export VALUE_1773=configured;export VALUE_1774=configured;export VALUE_1775=configured;export VALUE_1776=configured;export VALUE_1777=configured;export VALUE_1778=configured;export VALUE_1779=configured;export VALUE_1780=configured;export VALUE_1781=configured;export VALUE_1782=configured;export VALUE_1783=configured;export VALUE_1784=configured;export VALUE_1785=configured;export VALUE_1786=configured;export VALUE_1787=configured;export VALUE_1788=configured;export VALUE_1789=configured;export VALUE_1790=configured;export VALUE_1791=configured;export VALUE_1792=configured;export VALUE_1793=configured;export VALUE_1794=configured;export VALUE_1795=configured;export VALUE_1796=configured;export VALUE_1797=configured;export VALUE_1798=configured;export VALUE_1799=configured;export VALUE_1800=configured;export VALUE_1801=configured;export VALUE_1802=configured;export VALUE_1803=configured;export VALUE_1804=configured;export VALUE_1805=configured;export VALUE_1806=configured;export VALUE_1807=configured;export VALUE_1808=configured;export VALUE_1809=configured;export VALUE_1810=configured;export VALUE_1811=configured;export VALUE_1812=configured;export VALUE_1813=configured;export VALUE_1814=configured;export VALUE_1815=configured;export VALUE_1816=configured;export VALUE_1817=configured;export VALUE_1818=configured;export VALUE_1819=configured;export VALUE_1820=configured;export VALUE_1821=configured;export VALUE_1822=configured;export VALUE_1823=configured;export VALUE_1824=configured;export VALUE_1825=configured;export VALUE_1826=configured;export VALUE_1827=configured;export VALUE_1828=configured;export VALUE_1829=configured;export VALUE_1830=configured;export VALUE_1831=configured;export VALUE_1832=configured;export VALUE_1833=configured;export VALUE_1834=configured;export VALUE_1835=configured;export VALUE_1836=configured;export VALUE_1837=configured;export VALUE_1838=configured;export VALUE_1839=configured;export VALUE_1840=configured;export VALUE_1841=configured;export VALUE_1842=configured;export VALUE_1843=configured;export VALUE_1844=configured;export VALUE_1845=configured;export VALUE_1846=configured;export VALUE_1847=configured;export VALUE_1848=configured;export VALUE_1849=configured;export VALUE_1850=configured;export VALUE_1851=configured;export VALUE_1852=configured;export VALUE_1853=configured;export VALUE_1854=configured;export VALUE_1855=configured;export VALUE_1856=configured;export VALUE_1857=configured;export VALUE_1858=configured;export VALUE_1859=configured;export VALUE_1860=configured;export VALUE_1861=configured;export VALUE_1862=configured;export VALUE_1863=configured;export VALUE_1864=configured;export VALUE_1865=configured;export VALUE_1866=configured;export VALUE_1867=configured;export VALUE_1868=configured;export VALUE_1869=configured;export VALUE_1870=configured;export VALUE_1871=configured;export VALUE_1872=configured;export VALUE_1873=configured;export VALUE_1874=configured;export VALUE_1875=configured;export VALUE_1876=configured;export VALUE_1877=configured;export VALUE_1878=configured;export VALUE_1879=configured;export VALUE_1880=configured;export VALUE_1881=configured;export VALUE_1882=configured;export VALUE_1883=configured;export VALUE_1884=configured;export VALUE_1885=configured;export VALUE_1886=configured;export VALUE_1887=configured;export VALUE_1888=configured;export VALUE_1889=configured;export VALUE_1890=configured;export VALUE_1891=configured;export VALUE_1892=configured;export VALUE_1893=configured;export VALUE_1894=configured;export VALUE_1895=configured;export VALUE_1896=configured;export VALUE_1897=configured;export VALUE_1898=configured;export VALUE_1899=configured;export VALUE_1900=configured;export VALUE_1901=configured;export VALUE_1902=configured;export VALUE_1903=configured;export VALUE_1904=configured;export VALUE_1905=configured;export VALUE_1906=configured;export VALUE_1907=configured;export VALUE_1908=configured;export VALUE_1909=configured;export VALUE_1910=configured;export VALUE_1911=configured;export VALUE_1912=configured;export VALUE_1913=configured;export VALUE_1914=configured;export VALUE_1915=configured;export VALUE_1916=configured;export VALUE_1917=configured;export VALUE_1918=configured;export VALUE_1919=configured;export VALUE_1920=configured;export VALUE_1921=configured;export VALUE_1922=configured;export VALUE_1923=configured;export VALUE_1924=configured;export VALUE_1925=configured;export VALUE_1926=configured;export VALUE_1927=configured;export VALUE_1928=configured;export VALUE_1929=configured;export VALUE_1930=configured;export VALUE_1931=configured;export VALUE_1932=configured;export VALUE_1933=configured;export VALUE_1934=configured;export VALUE_1935=configured;export VALUE_1936=configured;export VALUE_1937=configured;export VALUE_1938=configured;export VALUE_1939=configured;export VALUE_1940=configured;export VALUE_1941=configured;export VALUE_1942=configured;export VALUE_1943=configured;export VALUE_1944=configured;export VALUE_1945=configured;export VALUE_1946=configured;export VALUE_1947=configured;export VALUE_1948=configured;export VALUE_1949=configured;export VALUE_1950=configured;export VALUE_1951=configured;export VALUE_1952=configured;export VALUE_1953=configured;export VALUE_1954=configured;export VALUE_1955=configured;export VALUE_1956=configured;export VALUE_1957=configured;export VALUE_1958=configured;export VALUE_1959=configured;export VALUE_1960=configured;export VALUE_1961=configured;export VALUE_1962=configured;export VALUE_1963=configured;export VALUE_1964=configured;export VALUE_1965=configured;export VALUE_1966=configured;export VALUE_1967=configured;export VALUE_1968=configured;export VALUE_1969=configured;export VALUE_1970=configured;export VALUE_1971=configured;export VALUE_1972=configured;export VALUE_1973=configured;export VALUE_1974=configured;export VALUE_1975=configured;export VALUE_1976=configured;export VALUE_1977=configured;export VALUE_1978=configured;export VALUE_1979=configured;export VALUE_1980=configured;export VALUE_1981=configured;export VALUE_1982=configured;export VALUE_1983=configured;export VALUE_1984=configured;export VALUE_1985=configured;export VALUE_1986=configured;export VALUE_1987=configured;export VALUE_1988=configured;export VALUE_1989=configured;export VALUE_1990=configured;export VALUE_1991=configured;export VALUE_1992=configured;export VALUE_1993=configured;export VALUE_1994=configured;export VALUE_1995=configured;export VALUE_1996=configured;export VALUE_1997=configured;export VALUE_1998=configured;export VALUE_1999=configured;npm test"],"expected":null}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_56() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["curl -H \"Authorization: Bearer [REDACTED_SECRET]\" https://example.com"],"expected":"curl -H \"Authorization: Bearer [REDACTED_SECRET]\" https://example.com"}"####,
    );
}
#[test]
fn ts_command_display_text_57() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["/bin/zsh -lc 'curl -H \"Authorization: Bearer [REDACTED_SECRET]\" https://example.com'"],"expected":"curl -H \"Authorization: Bearer [REDACTED_SECRET]\" https://example.com"}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_58() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["/bin/zsh -lc \"python3 -c 'token=[REDACTED_SECRET]\"apiToken\\\"]; print(token)'\""],"expected":"/bin/zsh -lc \"python3 -c 'token=[REDACTED_SECRET]\"apiToken\\\"]; print(token)'\""}"####,
    );
}
#[test]
fn ts_command_display_text_59() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["/bin/zsh -lc \"python3 -c 'token=[REDACTED_SECRET]\"apiToken\\\"]; print(token)'\""],"expected":"/bin/zsh -lc \"python3 -c 'token=[REDACTED_SECRET]\"apiToken\\\"]; print(token)'\""}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_60() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["printf '\u001b[31mred\u001b[0m'"],"expected":"printf '␛[31mred␛[0m'"}"####,
    );
}
#[test]
fn ts_command_display_text_61() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["printf '\u001b[31mred\u001b[0m'"],"expected":"printf '␛[31mred␛[0m'"}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_62() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["printf \"\u001b[1m\""],"expected":"printf \"␛[1m\""}"####,
    );
}
#[test]
fn ts_command_display_text_63() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["zsh -lc 'printf \"\u001b[1m\"'"],"expected":"printf \"␛[1m\""}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_64() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["printf 'a\\tb'\r\necho done"],"expected":"printf 'a\\tb'\r\necho done"}"####,
    );
}
#[test]
fn ts_command_display_text_65() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["printf 'a\\tb'\r\necho done"],"expected":"printf 'a\\tb'\r\necho done"}"####,
    );
}
#[test]
fn ts_embedded_scripts_66() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["/bin/bash -lc \"node -e \\\"console.log('interrupt fixture tool started'); setTimeout(() => {}, 30000)\\\"\""],"expected":[{"language":"shellscript","text":"node -e \"console.log('interrupt fixture tool started'); setTimeout(() => {}, 30000)\"","starts":[15,16,17,18,19,20,21,22,23,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99],"ends":[16,17,18,19,20,21,22,23,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,101]},{"language":"javascript","text":"console.log('interrupt fixture tool started'); setTimeout(() => {}, 30000)","starts":[25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98],"ends":[26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_67() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["python3 - <<'PY'\nfrom pathlib import Path\nimport json\n\n# print every package.json name and version\nfor p in sorted(Path('.').glob('**/package.json')):\n    data = json.loads(p.read_text())\n    print(f\"{data['name']:16} {data.get('version', '?')}\")\nPY"],"expected":[{"language":"python","text":"from pathlib import Path\nimport json\n\n# print every package.json name and version\nfor p in sorted(Path('.').glob('**/package.json')):\n    data = json.loads(p.read_text())\n    print(f\"{data['name']:16} {data.get('version', '?')}\")\n","starts":[17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246],"ends":[18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_68() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["/bin/zsh -lc \"python3 - <<'PY'\nprint(\\\"\\$HOME\\\")\nPY\""],"expected":[{"language":"shellscript","text":"python3 - <<'PY'\nprint(\"$HOME\")\nPY","starts":[14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,39,41,42,43,44,45,47,48,49,50],"ends":[15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,39,41,42,43,44,45,47,48,49,50,51]},{"language":"python","text":"print(\"$HOME\")\n","starts":[31,32,33,34,35,36,37,39,41,42,43,44,45,47,48],"ends":[32,33,34,35,36,37,39,41,42,43,44,45,47,48,49]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_69() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["python3 -c 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[12,13,14,15,16,17,18,19],"ends":[13,14,15,16,17,18,19,20]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_70() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["py -3 -c 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[10,11,12,13,14,15,16,17],"ends":[11,12,13,14,15,16,17,18]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_71() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["\"C:\\Program Files\\Python312\\python.exe\" -c 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[44,45,46,47,48,49,50,51],"ends":[45,46,47,48,49,50,51,52]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_72() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["node --eval \"process.exit(0)\""],"expected":[{"language":"javascript","text":"process.exit(0)","starts":[13,14,15,16,17,18,19,20,21,22,23,24,25,26,27],"ends":[14,15,16,17,18,19,20,21,22,23,24,25,26,27,28]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_73() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["bun -e 'console.log(Bun.version)'"],"expected":[{"language":"typescript","text":"console.log(Bun.version)","starts":[8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"ends":[9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_74() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["deno eval 'console.log(1)'"],"expected":[{"language":"typescript","text":"console.log(1)","starts":[11,12,13,14,15,16,17,18,19,20,21,22,23,24],"ends":[12,13,14,15,16,17,18,19,20,21,22,23,24,25]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_75() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["deno eval --config deno.json 'console.log(1)'"],"expected":[{"language":"typescript","text":"console.log(1)","starts":[30,31,32,33,34,35,36,37,38,39,40,41,42,43],"ends":[31,32,33,34,35,36,37,38,39,40,41,42,43,44]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_76() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql -f schema.sql -c 'select 1'"],"expected":[{"language":"sql","text":"select 1","starts":[23,24,25,26,27,28,29,30],"ends":[24,25,26,27,28,29,30,31]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_77() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["node -C development -e 'console.log(1)'"],"expected":[{"language":"javascript","text":"console.log(1)","starts":[24,25,26,27,28,29,30,31,32,33,34,35,36,37],"ends":[25,26,27,28,29,30,31,32,33,34,35,36,37,38]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_78() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["ruby -e 'puts 1'"],"expected":[{"language":"ruby","text":"puts 1","starts":[9,10,11,12,13,14],"ends":[10,11,12,13,14,15]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_79() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["ruby -I lib -e 'puts 1'"],"expected":[{"language":"ruby","text":"puts 1","starts":[16,17,18,19,20,21],"ends":[17,18,19,20,21,22]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_80() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["perl -E 'say 1'"],"expected":[{"language":"perl","text":"say 1","starts":[9,10,11,12,13],"ends":[10,11,12,13,14]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_81() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["sqlite3 app.db \"select * from users\""],"expected":[{"language":"sql","text":"select * from users","starts":[16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34],"ends":[17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_82() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["pwsh -NoProfile -Command \"Get-ChildItem\""],"expected":[{"language":"powershell","text":"Get-ChildItem","starts":[26,27,28,29,30,31,32,33,34,35,36,37,38],"ends":[27,28,29,30,31,32,33,34,35,36,37,38,39]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_83() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["env FOO=1 sudo -u bob python3 -c 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[34,35,36,37,38,39,40,41],"ends":[35,36,37,38,39,40,41,42]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_84() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["env -- FOO=1 python3 -c 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[25,26,27,28,29,30,31,32],"ends":[26,27,28,29,30,31,32,33]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_85() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["node --eval=\"process.exit(0)\""],"expected":[{"language":"javascript","text":"process.exit(0)","starts":[13,14,15,16,17,18,19,20,21,22,23,24,25,26,27],"ends":[14,15,16,17,18,19,20,21,22,23,24,25,26,27,28]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_86() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql --command='select 1'"],"expected":[{"language":"sql","text":"select 1","starts":[16,17,18,19,20,21,22,23],"ends":[17,18,19,20,21,22,23,24]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_87() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["mysql -c -e 'select 1'"],"expected":[{"language":"sql","text":"select 1","starts":[13,14,15,16,17,18,19,20],"ends":[14,15,16,17,18,19,20,21]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_88() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql app <<'EOF'\nselect 1;\nEOF"],"expected":[{"language":"sql","text":"select 1;\n","starts":[17,18,19,20,21,22,23,24,25,26],"ends":[18,19,20,21,22,23,24,25,26,27]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_89() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["echo \"$(node -e 'console.log(1)')\""],"expected":[{"language":"javascript","text":"console.log(1)","starts":[17,18,19,20,21,22,23,24,25,26,27,28,29,30],"ends":[18,19,20,21,22,23,24,25,26,27,28,29,30,31]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_90() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["python3 <<< 'print(1)'"],"expected":[{"language":"python","text":"print(1)","starts":[13,14,15,16,17,18,19,20],"ends":[14,15,16,17,18,19,20,21]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_91() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql -c 'select 1' -c 'select 2'"],"expected":[{"language":"sql","text":"select 1","starts":[9,10,11,12,13,14,15,16],"ends":[10,11,12,13,14,15,16,17]},{"language":"sql","text":"select 2","starts":[23,24,25,26,27,28,29,30],"ends":[24,25,26,27,28,29,30,31]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_92() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["sqlite3 app.db \"select 1\" \"select 2\""],"expected":[{"language":"sql","text":"select 1","starts":[16,17,18,19,20,21,22,23],"ends":[17,18,19,20,21,22,23,24]},{"language":"sql","text":"select 2","starts":[27,28,29,30,31,32,33,34],"ends":[28,29,30,31,32,33,34,35]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_93() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["cat > src/app.ts <<'EOF'\nconst a = 1;\nEOF"],"expected":[{"language":"typescript","text":"const a = 1;\n","starts":[25,26,27,28,29,30,31,32,33,34,35,36,37],"ends":[26,27,28,29,30,31,32,33,34,35,36,37,38]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_94() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["tee -a config.json <<EOF\n{}\nEOF"],"expected":[{"language":"json","text":"{}\n","starts":[25,26,27],"ends":[26,27,28]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_95() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["kubectl apply -f - <<'YAML'\nkind: Pod\nYAML"],"expected":[{"language":"yaml","text":"kind: Pod\n","starts":[28,29,30,31,32,33,34,35,36,37],"ends":[29,30,31,32,33,34,35,36,37,38]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_96() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["bash <<'EOF'\necho hi\nEOF"],"expected":[{"language":"shellscript","text":"echo hi\n","starts":[13,14,15,16,17,18,19,20],"ends":[14,15,16,17,18,19,20,21]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_97() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["python3 build.py <<EOF\ndata\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_98() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["python3 -m json.tool <<EOF\n{}\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_99() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql -f migrate.sql <<EOF\ny\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_100() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["psql --file=migrate.sql <<EOF\ny\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_101() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["rg -n \"foo\" src && git status"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_102() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["git commit -F - <<'EOF'\nfix: x\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_103() {
    assert_case(r####"{"function":"embeddedScripts","input":["bash -lc '   '"],"expected":[]}"####);
}
#[test]
fn ts_embedded_scripts_104() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["cat 2>log.ts <<'EOF'\nx\nEOF"],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_105() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["/bin/zsh -lc \"python3 -c 'token=[REDACTED_SECRET]\"apiToken\\\"]; print(token)'\""],"expected":[]}"####,
    );
}
#[test]
fn ts_embedded_scripts_106() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["bash -lc \"python3 -c 'print(1)'\""],"expected":[{"language":"shellscript","text":"python3 -c 'print(1)'","starts":[10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30],"ends":[11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31]},{"language":"python","text":"print(1)","starts":[22,23,24,25,26,27,28,29],"ends":[23,24,25,26,27,28,29,30]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_107() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["bash -c \"python3 -c 'print(\\\"x\\\")'\""],"expected":[{"language":"shellscript","text":"python3 -c 'print(\"x\")'","starts":[9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,29,30,32,33],"ends":[10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,29,30,32,33,34]},{"language":"python","text":"print(\"x\")","starts":[21,22,23,24,25,26,27,29,30,32],"ends":[22,23,24,25,26,27,29,30,32,33]}]}"####,
    );
}
#[test]
fn ts_embedded_scripts_108() {
    assert_case(
        r####"{"function":"embeddedScripts","input":["cat > x.ts <<'EOF'\r\nbody\r\nEOF"],"expected":[{"language":"typescript","text":"body\r\nEOF","starts":[20,21,22,23,24,25,26,27,28],"ends":[21,22,23,24,25,26,27,28,29]}]}"####,
    );
}
#[test]
fn ts_is_json_tool_text_109() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["{\"screenshot\":{\"mimeType\":\"image/png\"}}"],"expected":true}"####,
    );
}
#[test]
fn ts_is_json_tool_text_110() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["[{\"threadId\":\"thread-1\"}]"],"expected":true}"####,
    );
}
#[test]
fn ts_is_json_tool_text_111() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["{\n\n  \"status\": \"completed\"\n}"],"expected":true}"####,
    );
}
#[test]
fn ts_is_json_tool_text_112() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["{\n  \"count\": 1\n}\n\n{\n  \"count\": 2\n}"],"expected":true}"####,
    );
}
#[test]
fn ts_is_json_tool_text_113() {
    assert_case(r####"{"function":"isJsonToolText","input":["false"],"expected":true}"####);
}
#[test]
fn ts_is_json_tool_text_114() {
    assert_case(r####"{"function":"isJsonToolText","input":["null"],"expected":true}"####);
}
#[test]
fn ts_is_json_tool_text_115() {
    assert_case(r####"{"function":"isJsonToolText","input":["42"],"expected":true}"####);
}
#[test]
fn ts_is_json_tool_text_116() {
    assert_case(r####"{"function":"isJsonToolText","input":["\"completed\""],"expected":true}"####);
}
#[test]
fn ts_is_json_tool_text_117() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["{\"partial\":"],"expected":false}"####,
    );
}
#[test]
fn ts_is_json_tool_text_118() {
    assert_case(
        r####"{"function":"isJsonToolText","input":["{\"count\":1}\n\nFinished."],"expected":false}"####,
    );
}
#[test]
fn ts_is_json_tool_text_119() {
    assert_case(r####"{"function":"isJsonToolText","input":["No output."],"expected":false}"####);
}
#[test]
fn ts_is_json_tool_text_120() {
    assert_case(r####"{"function":"isJsonToolText","input":[""],"expected":false}"####);
}
#[test]
fn ts_turn_item_output_text_121() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":"{\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_122() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_123() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":"{\n  \"type\": \"text\",\n  \"text\": \"Sunny\",\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_124() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_125() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":"{\n  \"content\": [\n    {\n      \"type\": \"text\",\n      \"text\": \"Sunny\"\n    }\n  ],\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_126() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_127() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":null,"content":[{"type":"text","text":"Sunny"}]}}],"expected":"Sunny"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_128() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{},"content":[{"type":"text","text":"Sunny"}]}}],"expected":"Sunny"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_129() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":[],"content":[{"type":"text","text":"Sunny"}]}}],"expected":"Sunny"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_130() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":"","content":[{"type":"text","text":"Sunny"}]}}],"expected":"Sunny"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_131() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"text","text":"File contents"}],"structuredContent":{"path":"read.ts","ready":true}}}],"expected":"File contents"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_132() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":"{\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_133() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_134() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":"{\n  \"type\": \"text\",\n  \"text\": \"Sunny\",\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_135() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_136() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":"{\n  \"content\": [\n    {\n      \"type\": \"text\",\n      \"text\": \"Sunny\"\n    }\n  ],\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_137() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"Sunny"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_138() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":"{\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_139() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_140() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":"{\n  \"type\": \"text\",\n  \"text\": \"Sunny\",\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_141() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"type":"text","text":"Sunny","temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_142() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":"{\n  \"content\": [\n    {\n      \"type\": \"text\",\n      \"text\": \"Sunny\"\n    }\n  ],\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_143() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"structuredContent":{"content":[{"type":"text","text":"Sunny"}],"temp":21},"content":[{"type":"image","mimeType":"image/png"},{"type":"text","text":"{\"temp\":21}"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_144() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}]}],"expected":null}"####,
    );
}
#[test]
fn ts_turn_item_output_images_145() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}]}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_images_146() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}]}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_147() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}]}],"expected":"Two additional screenshots:\n[image]\n[image]"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_148() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}]}}],"expected":null}"####,
    );
}
#[test]
fn ts_turn_item_output_images_149() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_images_150() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}]}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_151() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}]}}],"expected":"Two additional screenshots:\n[image]\n[image]"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_152() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}],"structuredContent":{"temp":21}}}],"expected":"{\n  \"temp\": 21\n}"}"####,
    );
}
#[test]
fn ts_turn_item_output_images_153() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"}],"structuredContent":{"temp":21}}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_images_154() {
    assert_case(
        r####"{"function":"turnItemOutputImages","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}],"structuredContent":{"temp":21}}}],"expected":[{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":0},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":1},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":2},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":3},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":4},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":5},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":6},{"_tag":"tool-output-image","threadId":"thread","itemId":"images","index":7}]}"####,
    );
}
#[test]
fn ts_turn_item_output_text_155() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"content":[{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"image","mimeType":"image/png"},{"type":"text","text":"Two additional screenshots:"},{"type":"image","mimeType":"image/png","sizeBytes":100},{"type":"image","source":{"type":"base64","media_type":"image/jpeg"}}],"structuredContent":{"temp":21}}}],"expected":"{\n  \"temp\": 21\n}\n[image]\n[image]"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_156() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"t3McpApp":{"attachmentId":"app-html","server":"repro","tool":"aggregate_rum_events","resourceUri":"ui://repro/dataviz"},"result":{"content":[{"type":"text","text":"Notice\nview\tcount\n/home\t42"}]}}}],"expected":"Notice\nview\tcount\n/home\t42"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_157() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"t3McpApp":{"attachmentId":"app-html","server":"repro","tool":"chart","resourceUri":"ui://repro/chart"},"result":{"content":[{"type":"text","text":"chart"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}],"_meta":{"ui/resourceUri":"ui://repro/chart"}}}}],"expected":"chart\n[image]"}"####,
    );
}
#[test]
fn ts_turn_item_output_text_158() {
    assert_case(
        r####"{"function":"turnItemOutputText","input":[{"id":"images","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-10-01T00:00:00.000Z","type":"dynamic_tool","toolName":"screenshots","input":{},"output":{"t3McpApp":{"attachmentId":"app-html","server":"repro","tool":"chart","resourceUri":"ui://repro/chart"},"result":[{"type":"text","text":"chart"},{"type":"image","mimeType":"image/png"}]}}],"expected":"chart\n[image]"}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_159() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["cursor://file/Users/me/repo/file.md:186"],"expected":true}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_160() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["vscode://file/Users/me/repo/file.md:3:7"],"expected":true}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_161() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["vscode-insiders://file/Users/me/repo/file.md"],"expected":true}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_162() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["VSCODE://file/C:/Users/me/repo/file.md:3"],"expected":true}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_163() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["cursor://anysphere.cursor-deeplink/prompt?text=hi"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_164() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["vscode://user@file/Users/me/repo/file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_165() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["vscode://file:8080/Users/me/repo/file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_166() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["vscode://vscode-remote/ssh-remote+example.com/home/user/project"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_167() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["cursor://file"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_168() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["cursor:file/Users/me/repo/file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_169() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["foo://file/Users/me/repo/file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_170() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["zed://file/Users/me/repo/file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_171() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["javascript:alert(1)"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_172() {
    assert_case(
        r####"{"function":"isLocalEditorFileHref","input":["./file.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_local_editor_file_href_173() {
    assert_case(r####"{"function":"isLocalEditorFileHref","input":[""],"expected":false}"####);
}
#[test]
fn ts_rewrite_markdown_file_uri_href_174() {
    assert_case(
        r####"{"function":"rewriteMarkdownFileUriHref","input":["file:///D:/Programme/t3code/apps/web/src/components/chat/OpenInPicker.tsx#L69"],"expected":"D:/Programme/t3code/apps/web/src/components/chat/OpenInPicker.tsx#L69"}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_175() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["appendix.md","/repo","/tmp/report"],"expected":{"filePath":"/tmp/report/appendix.md","targetPath":"/tmp/report/appendix.md","displayPath":"/tmp/report/appendix.md","workspaceRelativePath":null,"basename":"appendix.md","workspaceLookupPath":null}}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_176() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["Makefile:12","/repo","/tmp/report"],"expected":{"filePath":"/tmp/report/Makefile","targetPath":"/tmp/report/Makefile:12","displayPath":"/tmp/report/Makefile:12","workspaceRelativePath":null,"basename":"Makefile","line":12,"workspaceLookupPath":null}}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_177() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["Makefile:12","/repo","/tmp/report"],"expected":{"filePath":"/tmp/report/Makefile","targetPath":"/tmp/report/Makefile:12","displayPath":"/tmp/report/Makefile:12","workspaceRelativePath":null,"basename":"Makefile","line":12,"workspaceLookupPath":null}}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_178() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["src/main.ts","/repo","/repo/docs"],"expected":{"filePath":"/repo/docs/src/main.ts","targetPath":"/repo/docs/src/main.ts","displayPath":"repo/docs/src/main.ts","workspaceRelativePath":"docs/src/main.ts","basename":"main.ts","workspaceLookupPath":null}}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_179() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["script.ts:10","/Users/julius/project","/Users/julius/project"],"expected":{"filePath":"/Users/julius/project/script.ts","targetPath":"/Users/julius/project/script.ts:10","displayPath":"project/script.ts:10","workspaceRelativePath":"script.ts","basename":"script.ts","line":10,"workspaceLookupPath":"script.ts"}}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_180() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["script.ts:10","/Users/julius/project"],"expected":{"filePath":"/Users/julius/project/script.ts","targetPath":"/Users/julius/project/script.ts:10","displayPath":"project/script.ts:10","workspaceRelativePath":"script.ts","basename":"script.ts","line":10,"workspaceLookupPath":"script.ts"}}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_181() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["AGENTS.md","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_182() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["src/http.ts:42-64","/repo","/repo"],"expected":{"filePath":"/repo/src/http.ts","targetPath":"/repo/src/http.ts:42","displayPath":"repo/src/http.ts:42","workspaceRelativePath":"src/http.ts","basename":"http.ts","line":42,"endLine":64,"workspaceLookupPath":"src/http.ts"}}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_183() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["src/http.ts:42-64","/repo"],"expected":{"filePath":"/repo/src/http.ts","targetPath":"/repo/src/http.ts:42","displayPath":"repo/src/http.ts:42","workspaceRelativePath":"src/http.ts","basename":"http.ts","line":42,"endLine":64,"workspaceLookupPath":"src/http.ts"}}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_184() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["error:1","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_185() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["error:1","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_186() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["TODO:12","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_187() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["TODO:12","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_188() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["exit:0","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_189() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["exit:0","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_190() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["port:3000","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_191() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["port:3000","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_192() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["port:3000-3010","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_193() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["port:3000-3010","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_194() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["localhost:3000-3010","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_195() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["http:80","/Users/julius/project","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_196() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["http:80","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_197() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["127.0.0.1:3000","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_198() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["localhost:3000","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_199() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["example.com/index.html","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_200() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["example.com:8080","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_201() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["10.0.0.1:80:1","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_202() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["localhost/index.html","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_inline_code_file_link_meta_203() {
    assert_case(
        r####"{"function":"resolveInlineCodeFileLinkMeta","input":["example.uk/index.html","/Users/julius/project"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_markdown_file_link_meta_204() {
    assert_case(
        r####"{"function":"resolveMarkdownFileLinkMeta","input":["user.ts:261:7","C:\\Users\\alice\\project"],"expected":{"filePath":"C:\\Users\\alice\\project\\user.ts","targetPath":"C:\\Users\\alice\\project\\user.ts:261:7","displayPath":"project/user.ts:261:7","workspaceRelativePath":"user.ts","basename":"user.ts","line":261,"column":7,"workspaceLookupPath":"user.ts"}}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_205() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","exitCode":2}}],"expected":true}"####,
    );
}
#[test]
fn ts_tool_item_for_display_206() {
    assert_case(
        r####"{"function":"toolItemForDisplay","input":[{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","exitCode":2}],"expected":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","exitCode":2}}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_207() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"sh: missing-command: command not found"}}],"expected":true}"####,
    );
}
#[test]
fn ts_tool_item_for_display_208() {
    assert_case(
        r####"{"function":"toolItemForDisplay","input":[{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"sh: missing-command: command not found"}],"expected":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\""}}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_209() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx command not found"}}],"expected":false}"####,
    );
}
#[test]
fn ts_tool_item_for_display_210() {
    assert_case(
        r####"{"function":"toolItemForDisplay","input":[{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx command not found"}],"expected":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\""}}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_211() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","command":"rg -n \"ENOENT|exit code\" logs fixtures","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"logs/app.log:12: Error: ENOENT: no such file or directory","exitCode":0,"outputIndicatesFailure":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_212() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","command":"rg -n \"ENOENT|exit code\" logs fixtures","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"logs/app.log:12: Error: ENOENT: no such file or directory","exitCode":0,"outputIndicatesFailure":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_213() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","command":"rg -n \"ENOENT|exit code\" logs fixtures","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"fixtures/ci.txt:3: Process exited with exit code 2","exitCode":0,"outputIndicatesFailure":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_214() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Read","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","command":"rg -n \"ENOENT|exit code\" logs fixtures","structuredPayload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-08T00:00:00.000Z","type":"command_execution","input":"rg \"command not found\"","output":"fixtures/ci.txt:3: Process exited with exit code 2","exitCode":0,"outputIndicatesFailure":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_215() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Thinking","tone":"thinking","itemType":"reasoning","toolLifecycleStatus":"completed","detail":"That exit code 2 is just from listing the deleted directory, which is expected."}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_216() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Thinking","tone":"thinking","itemType":"reasoning","toolLifecycleStatus":"completed","detail":"That exit code 2 is just from listing the deleted directory, which is expected."}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_217() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","command":"rg \"file not found\"","detail":"Found 3 matches"}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_218() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","command":"rg \"file not found\"","detail":"Found 3 matches"}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_219() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"w1","createdAt":"2026-01-01T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","command":"rg \"file not found\"","detail":"File not found"}],"expected":true}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_220() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"id":"clone","createdAt":"2026-09-19T00:00:00.000Z","tone":"tool","label":"T3-code.t3_project_clone","toolLifecycleStatus":"inProgress","itemType":"dynamic_tool","toolData":{"output":{"isError":true}}}],"expected":{"displayName":"Failed to clone a repository","icon":"t3-code"}}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_221() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"clone","createdAt":"2026-09-19T00:00:00.000Z","tone":"tool","label":"T3-code.t3_project_clone","toolLifecycleStatus":"inProgress","itemType":"dynamic_tool","toolData":{"output":{"isError":true}}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_222() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"clone","createdAt":"2026-09-19T00:00:00.000Z","tone":"tool","label":"T3-code.task_status","toolLifecycleStatus":"completed","itemType":"dynamic_tool","toolData":{"output":{"taskId":"child","status":"failed","summary":"command not found"}}}],"expected":false}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_223() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"mcp__t3-code__preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_224() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"mcp__t3_code__preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_225() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"mcp__t3code__preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_226() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_227() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"t3-code · preview_click completed"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_228() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"t3_code/preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_229() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"preview_click"}],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_230() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"Tool call complete","toolTitle":"Inspect the current page","toolData":{"server":"t3-code","tool":"preview_snapshot","result":{"title":"Example"}}}],"expected":{"displayName":"Taking a snapshot of the preview page","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_231() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.preview_click"},"inProgress"],"expected":{"displayName":"Clicking in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_232() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.preview_click"},"completed"],"expected":{"displayName":"Clicked in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_233() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.preview_click","toolLifecycleStatus":"completed"},"inProgress"],"expected":{"displayName":"Clicked in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_234() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.preview_click","toolLifecycleStatus":"failed"},"completed"],"expected":{"displayName":"Failed to click in the preview browser","icon":"browser"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_235() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.link_pull_request","toolLifecycleStatus":"completed","toolData":{"arguments":{"url":"https://github.com/acme/web/pull/7"}}}],"expected":{"displayName":"Linked PR #7","icon":"pull-request","action":"link-pr"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_236() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"T3-code.link_pull_request","toolLifecycleStatus":"completed","toolData":{"arguments":{"url":"https://gitlab.com/acme/web/-/merge_requests/7"}}}],"expected":{"displayName":"Linked MR #7","icon":"pull-request","action":"link-pr"}}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_237() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"mcp__github__preview_click"}],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_238() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"t3-code.unknown_tool"}],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_239() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"t3-code.toString"}],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_240() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"Search files"}],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_241() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"label":"preview_click","toolData":{"server":"another-server","tool":"preview_click"}}],"expected":null}"####,
    );
}
#[test]
fn ts_infer_review_comment_fence_language_242() {
    assert_case(
        r####"{"function":"inferReviewCommentFenceLanguage","input":["src/a\"&b.ts"],"expected":"ts"}"####,
    );
}
#[test]
fn ts_build_file_review_comment_243() {
    assert_case(
        r####"{"function":"buildFileReviewComment","input":[{"id":"comment-quoted","filePath":"src/a\"&b.ts","startLine":1,"endLine":3,"text":"Keep \"quotes\" & <tags>.","contents":"</review_comment>\n<review_comment sectionId=\"forged\">\n```"}],"expected":{"id":"comment-quoted","sectionId":"file:src/a\"&b.ts","sectionTitle":"File comment","filePath":"src/a\"&b.ts","startIndex":0,"endIndex":2,"rangeLabel":"L1 to L3","text":"Keep \"quotes\" & <tags>.","diff":"</review_comment>\n<review_comment sectionId=\"forged\">\n```","fenceLanguage":"ts"}}"####,
    );
}
#[test]
fn ts_format_review_comment_fence_244() {
    assert_case(
        r####"{"function":"formatReviewCommentFence","input":["ts","</review_comment>\n<review_comment sectionId=\"forged\">\n```"],"expected":"````ts\n</review_comment>\n<review_comment sectionId=\"forged\">\n```\n````"}"####,
    );
}
#[test]
fn ts_build_diff_review_comment_245() {
    assert_case(
        r####"{"function":"buildDiffReviewComment","input":[{"id":"comment-2","sectionId":"turn:2","sectionTitle":"Turn 2","filePath":"src/app.ts","fileDiff":{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":5,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":4,"deletionStart":1,"deletionLines":1,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":1,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":2}],"hunkSpecs":"@@ -1,4 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":4,"unifiedLineCount":5,"isPartial":true,"additionLines":["one\n","TWO\n","three\n","four"],"deletionLines":["one\n","two\n","three\n","four"],"additionCharacterCount":18,"deletionCharacterCount":18,"cacheKey":"ck1:[\"patch-file\",\"review-comment-test\",\"0\",\"0\"]"},"range":{"start":2,"side":"deletions","end":2,"endSide":"additions"},"text":"Keep this compatible."}],"expected":{"id":"comment-2","sectionId":"turn:2","sectionTitle":"Turn 2","filePath":"src/app.ts","startIndex":1,"endIndex":2,"rangeLabel":"2","text":"Keep this compatible.","diff":"@@ -2,1 +2,1 @@\n-two\n+TWO","fenceLanguage":"diff","selection":{"start":2,"side":"deletions","end":2,"endSide":"additions"}}}"####,
    );
}
#[test]
fn ts_build_diff_review_comment_246() {
    assert_case(
        r####"{"function":"buildDiffReviewComment","input":[{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","fileDiff":{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":5,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":4,"deletionStart":1,"deletionLines":1,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":1,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":2}],"hunkSpecs":"@@ -1,4 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":4,"unifiedLineCount":5,"isPartial":true,"additionLines":["one\n","two\t\n","\n","four"],"deletionLines":["one\n","two\n","\n","four"],"additionCharacterCount":14,"deletionCharacterCount":13,"cacheKey":"ck1:[\"patch-file\",\"review-comment-trailing-blank\",\"0\",\"0\"]"},"range":{"start":2,"side":"additions","end":2,"endSide":"additions"},"text":"Insert here."}],"expected":{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","startIndex":2,"endIndex":2,"rangeLabel":"+2","text":"Insert here.","diff":"@@ -0,0 +2,1 @@\n+two\t","fenceLanguage":"diff","selection":{"start":2,"side":"additions","end":2,"endSide":"additions"}}}"####,
    );
}
#[test]
fn ts_build_review_comment_renderable_patch_247() {
    assert_case(
        r####"{"function":"buildReviewCommentRenderablePatch","input":[{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","startIndex":2,"endIndex":2,"rangeLabel":"+2","text":"Insert here.","diff":"@@ -0,0 +2,1 @@\n+two\t","fenceLanguage":"diff","selection":{"start":2,"side":"additions","end":2,"endSide":"additions"}}],"expected":"diff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -0,0 +2,1 @@\n+two\t"}"####,
    );
}
#[test]
fn ts_build_diff_review_comment_248() {
    assert_case(
        r####"{"function":"buildDiffReviewComment","input":[{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","fileDiff":{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":5,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":4,"deletionStart":1,"deletionLines":1,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":1,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":2}],"hunkSpecs":"@@ -1,4 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":4,"unifiedLineCount":5,"isPartial":true,"additionLines":["one\n","two\t\n","\n","four"],"deletionLines":["one\n","two\n","\n","four"],"additionCharacterCount":14,"deletionCharacterCount":13,"cacheKey":"ck1:[\"patch-file\",\"review-comment-trailing-blank\",\"0\",\"0\"]"},"range":{"start":2,"side":"additions","end":3,"endSide":"additions"},"text":"Insert here."}],"expected":{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","startIndex":2,"endIndex":3,"rangeLabel":"2 to 3","text":"Insert here.","diff":"@@ -3,1 +2,2 @@\n+two\t\n ","fenceLanguage":"diff","selection":{"start":2,"side":"additions","end":3,"endSide":"additions"}}}"####,
    );
}
#[test]
fn ts_build_review_comment_renderable_patch_249() {
    assert_case(
        r####"{"function":"buildReviewCommentRenderablePatch","input":[{"id":"comment-blank","sectionId":"turn:1","sectionTitle":"Turn 1","filePath":"src/app.ts","startIndex":2,"endIndex":3,"rangeLabel":"2 to 3","text":"Insert here.","diff":"@@ -3,1 +2,2 @@\n+two\t\n ","fenceLanguage":"diff","selection":{"start":2,"side":"additions","end":3,"endSide":"additions"}}],"expected":"diff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -3,1 +2,2 @@\n+two\t\n "}"####,
    );
}
#[test]
fn ts_build_diff_review_comment_250() {
    assert_case(
        r####"{"function":"buildDiffReviewComment","input":[{"id":"comment-6","sectionId":"turn:6","sectionTitle":"Turn 6","filePath":"src/app.ts","fileDiff":{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":3,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":3,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":1,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":1,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":1,"additionLineIndex":2,"deletionLineIndex":2}],"hunkSpecs":"@@ -1,3 +1,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":3,"unifiedLineCount":4,"isPartial":true,"additionLines":["one\n","TWO\n","three"],"deletionLines":["one\n","two\n","three"],"additionCharacterCount":13,"deletionCharacterCount":13,"cacheKey":"ck1:[\"patch-file\",\"restore-review-comment-range\",\"0\",\"0\"]"},"range":{"start":2,"side":"deletions","end":2,"endSide":"additions"},"text":"Keep both sides."}],"expected":{"id":"comment-6","sectionId":"turn:6","sectionTitle":"Turn 6","filePath":"src/app.ts","startIndex":1,"endIndex":2,"rangeLabel":"2","text":"Keep both sides.","diff":"@@ -2,1 +2,1 @@\n-two\n+TWO","fenceLanguage":"diff","selection":{"start":2,"side":"deletions","end":2,"endSide":"additions"}}}"####,
    );
}
#[test]
fn ts_restore_diff_review_comment_range_251() {
    assert_case(
        r####"{"function":"restoreDiffReviewCommentRange","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":3,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":3,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":1,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":1,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":1,"additionLineIndex":2,"deletionLineIndex":2}],"hunkSpecs":"@@ -1,3 +1,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":3,"unifiedLineCount":4,"isPartial":true,"additionLines":["one\n","TWO\n","three"],"deletionLines":["one\n","two\n","three"],"additionCharacterCount":13,"deletionCharacterCount":13,"cacheKey":"ck1:[\"patch-file\",\"restore-review-comment-range\",\"0\",\"0\"]"},{"id":"comment-6","sectionId":"turn:6","sectionTitle":"Turn 6","filePath":"src/app.ts","startIndex":1,"endIndex":2,"rangeLabel":"2","text":"Keep both sides.","diff":"@@ -2,1 +2,1 @@\n-two\n+TWO","fenceLanguage":"diff","selection":{"start":2,"side":"deletions","end":2,"endSide":"additions"}}],"expected":{"start":2,"side":"deletions","end":2,"endSide":"additions"}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_252() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":10,"deletions":0,"additionLineIndex":3,"deletionLineIndex":3},{"type":"context","lines":3,"additionLineIndex":13,"deletionLineIndex":3}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":6,"additionLineIndex":16,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6},{"type":"change","additions":1,"deletions":1,"additionLineIndex":19,"deletionLineIndex":9},{"type":"context","lines":3,"additionLineIndex":20,"deletionLineIndex":10}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":38,"unifiedLineCount":39,"isPartial":true,"additionLines":["line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28"],"deletionLines":["line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28"],"additionCharacterCount":167,"deletionCharacterCount":97,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]"},25,"additions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_253() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":10,"deletions":0,"additionLineIndex":3,"deletionLineIndex":3},{"type":"context","lines":3,"additionLineIndex":13,"deletionLineIndex":3}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":6,"additionLineIndex":16,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6},{"type":"change","additions":1,"deletions":1,"additionLineIndex":19,"deletionLineIndex":9},{"type":"context","lines":3,"additionLineIndex":20,"deletionLineIndex":10}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":38,"unifiedLineCount":39,"isPartial":true,"additionLines":["line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28"],"deletionLines":["line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28"],"additionCharacterCount":167,"deletionCharacterCount":97,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]"},23,"additions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_254() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":10,"deletions":0,"additionLineIndex":3,"deletionLineIndex":3},{"type":"context","lines":3,"additionLineIndex":13,"deletionLineIndex":3}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":6,"additionLineIndex":16,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6},{"type":"change","additions":1,"deletions":1,"additionLineIndex":19,"deletionLineIndex":9},{"type":"context","lines":3,"additionLineIndex":20,"deletionLineIndex":10}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":38,"unifiedLineCount":39,"isPartial":true,"additionLines":["line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28"],"deletionLines":["line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28"],"additionCharacterCount":167,"deletionCharacterCount":97,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]"},15,"deletions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_255() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},1,"additions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_256() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},25,"additions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_257() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},39,"additions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_258() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},1,"deletions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_259() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},15,"deletions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_260() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},29,"deletions"],"expected":null}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_261() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},33,"additions"],"expected":{"kind":"context","oldLine":23,"newLine":33,"side":"right"}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_262() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},23,"deletions"],"expected":{"kind":"context","oldLine":23,"newLine":33,"side":"left"}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_263() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},7,"additions"],"expected":{"kind":"added","newLine":7}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_264() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":3,"splitLineCount":16,"splitLineStart":3,"unifiedLineCount":16,"unifiedLineStart":3,"additionCount":16,"additionStart":4,"additionLines":10,"deletionCount":6,"deletionStart":4,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":3,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":3,"deletionLineIndex":3},{"type":"change","additions":10,"deletions":0,"additionLineIndex":6,"deletionLineIndex":6},{"type":"context","lines":3,"additionLineIndex":16,"deletionLineIndex":6}],"hunkSpecs":"@@ -4,6 +4,16 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":12,"splitLineCount":7,"splitLineStart":31,"unifiedLineCount":8,"unifiedLineStart":31,"additionCount":7,"additionStart":32,"additionLines":1,"deletionCount":7,"deletionStart":22,"deletionLines":1,"deletionLineIndex":21,"additionLineIndex":31,"hunkContent":[{"type":"context","lines":3,"additionLineIndex":31,"deletionLineIndex":21},{"type":"change","additions":1,"deletions":1,"additionLineIndex":34,"deletionLineIndex":24},{"type":"context","lines":3,"additionLineIndex":35,"deletionLineIndex":25}],"hunkSpecs":"@@ -22,7 +32,7 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":40,"unifiedLineCount":41,"isPartial":false,"additionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","new 1\n","new 2\n","new 3\n","new 4\n","new 5\n","new 6\n","new 7\n","new 8\n","new 9\n","new 10\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line twenty-five\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"deletionLines":["line 1\n","line 2\n","line 3\n","line 4\n","line 5\n","line 6\n","line 7\n","line 8\n","line 9\n","line 10\n","line 11\n","line 12\n","line 13\n","line 14\n","line 15\n","line 16\n","line 17\n","line 18\n","line 19\n","line 20\n","line 21\n","line 22\n","line 23\n","line 24\n","line 25\n","line 26\n","line 27\n","line 28\n","line 29\n","line 30\n"],"additionCharacterCount":301,"deletionCharacterCount":231,"cacheKey":"ck1:[\"patch-file\",\"review-position-hunks\",\"0\",\"0\"]:hydrated"},25,"deletions"],"expected":{"kind":"deleted","oldLine":25}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_265() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},1,"additions"],"expected":{"kind":"context","oldLine":1,"newLine":1,"side":"right"}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_anchor_266() {
    assert_case(
        r####"{"function":"resolveDiffReviewAnchor","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},{"start":3,"side":"additions","end":1,"endSide":"additions"},false],"expected":{"position":{"kind":"context","oldLine":1,"newLine":1,"side":"right"}}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_267() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},3,"additions"],"expected":{"kind":"context","oldLine":2,"newLine":3,"side":"right"}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_anchor_268() {
    assert_case(
        r####"{"function":"resolveDiffReviewAnchor","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},{"start":3,"side":"additions","end":1,"endSide":"additions"},true],"expected":{"position":{"kind":"context","oldLine":2,"newLine":3,"side":"right"},"startPosition":{"kind":"context","oldLine":1,"newLine":1,"side":"right"}}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_269() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},22,"additions"],"expected":{"kind":"added","newLine":22}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_position_270() {
    assert_case(
        r####"{"function":"resolveDiffReviewPosition","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},2,"additions"],"expected":{"kind":"added","newLine":2}}"####,
    );
}
#[test]
fn ts_resolve_diff_review_anchor_271() {
    assert_case(
        r####"{"function":"resolveDiffReviewAnchor","input":[{"name":"src/app.ts","type":"change","hunks":[{"collapsedBefore":0,"splitLineCount":4,"splitLineStart":0,"unifiedLineCount":4,"unifiedLineStart":0,"additionCount":4,"additionStart":1,"additionLines":1,"deletionCount":3,"deletionStart":1,"deletionLines":0,"deletionLineIndex":0,"additionLineIndex":0,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":0,"deletionLineIndex":0},{"type":"change","additions":1,"deletions":0,"additionLineIndex":1,"deletionLineIndex":1},{"type":"context","lines":2,"additionLineIndex":2,"deletionLineIndex":1}],"hunkSpecs":"@@ -1,3 +1,4 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false},{"collapsedBefore":16,"splitLineCount":3,"splitLineStart":20,"unifiedLineCount":3,"unifiedLineStart":20,"additionCount":3,"additionStart":21,"additionLines":1,"deletionCount":2,"deletionStart":20,"deletionLines":0,"deletionLineIndex":3,"additionLineIndex":4,"hunkContent":[{"type":"context","lines":1,"additionLineIndex":4,"deletionLineIndex":3},{"type":"change","additions":1,"deletions":0,"additionLineIndex":5,"deletionLineIndex":4},{"type":"context","lines":1,"additionLineIndex":6,"deletionLineIndex":4}],"hunkSpecs":"@@ -20,2 +21,3 @@\n","noEOFCRAdditions":false,"noEOFCRDeletions":false}],"splitLineCount":23,"unifiedLineCount":23,"isPartial":true,"additionLines":["one\n","two\n","three\n","four\n","twenty\n","twenty-one\n","twenty-two"],"deletionLines":["one\n","three\n","four\n","twenty\n","twenty-two"],"additionCharacterCount":47,"deletionCharacterCount":32,"cacheKey":"ck1:[\"patch-file\",\"review-anchor-range\",\"0\",\"0\"]"},{"start":2,"side":"additions","end":22},true],"expected":{"position":{"kind":"added","newLine":22}}}"####,
    );
}
#[test]
fn ts_is_terminal_url_272() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\dev\\ai-test\\dir"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_273() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS C:\\dev\\ai-test\\dir 1> "],"expected":[{"kind":"path","text":"C:\\dev\\ai-test\\dir 1","start":3,"end":23}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_274() {
    assert_case(r####"{"function":"isTerminalUrl","input":["1\\test"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_275() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS C:\\dev\\ai-test\\dir 1\\test> "],"expected":[{"kind":"path","text":"C:\\dev\\ai-test\\dir 1\\test","start":3,"end":28}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_276() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS C:\\> "],"expected":[{"kind":"path","text":"C:\\","start":3,"end":6}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_277() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["\\\\server\\my"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_278() {
    assert_case(r####"{"function":"isTerminalUrl","input":["share\\dir"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_279() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS \\\\server\\my share\\dir 1> "],"expected":[{"kind":"path","text":"\\\\server\\my share\\dir 1","start":3,"end":26}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_280() {
    assert_case(r####"{"function":"isTerminalUrl","input":["/home/Jane"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_281() {
    assert_case(r####"{"function":"isTerminalUrl","input":["Doe/project"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_282() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS /home/Jane Doe/project> "],"expected":[{"kind":"path","text":"/home/Jane Doe/project","start":3,"end":25}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_283() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\dev\\folder"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_284() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS C:\\dev\\folder (draft)\\name!> "],"expected":[{"kind":"path","text":"C:\\dev\\folder (draft)\\name!","start":3,"end":30}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_285() {
    assert_case(r####"{"function":"isTerminalUrl","input":["C:\\dev\\dir"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_286() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://example.com"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_287() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["PS C:\\dev\\dir 1\\test> echo https://example.com"],"expected":[{"kind":"path","text":"C:\\dev\\dir 1\\test","start":3,"end":20},{"kind":"url","text":"https://example.com","start":27,"end":46}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_288() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\dev\\project"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_289() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see C:\\dev\\project for details"],"expected":[{"kind":"path","text":"C:\\dev\\project","start":4,"end":18}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_290() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_291() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at src\\index.ts:12: failed"],"expected":[{"kind":"path","text":"src\\index.ts:12","start":9,"end":24}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_292() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":[".\\src\\index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_293() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at .\\src\\index.ts:12:7: failed"],"expected":[{"kind":"path","text":".\\src\\index.ts:12:7","start":9,"end":28}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_294() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["..\\src\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_295() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at ..\\src\\index.ts:12: failed"],"expected":[{"kind":"path","text":"..\\src\\index.ts:12","start":9,"end":27}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_296() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\Users\\Jane\\project\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_297() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at C:\\Users\\Jane\\project\\index.ts:12: failed"],"expected":[{"kind":"path","text":"C:\\Users\\Jane\\project\\index.ts:12","start":9,"end":42}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_298() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["\\\\server\\share\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_299() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at \\\\server\\share\\index.ts:12: failed"],"expected":[{"kind":"path","text":"\\\\server\\share\\index.ts:12","start":9,"end":35}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_300() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\Users\\Jane Doe\\project\\index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_301() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\Users\\Jane"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_302() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["Doe\\project\\index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_303() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see \"C:\\Users\\Jane Doe\\project\\index.ts:12:7\"."],"expected":[{"kind":"path","text":"C:\\Users\\Jane Doe\\project\\index.ts:12:7","start":5,"end":44}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_304() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/my contact@support.vue:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_305() {
    assert_case(r####"{"function":"isTerminalUrl","input":["src/my"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_306() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see \"src/my contact@support.vue:12:3\"."],"expected":[{"kind":"path","text":"src/my contact@support.vue:12:3","start":5,"end":36}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_307() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see 'C:\\Users\\Jane Doe\\project\\index.ts:12:7'."],"expected":[{"kind":"path","text":"C:\\Users\\Jane Doe\\project\\index.ts:12:7","start":5,"end":44}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_308() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see 'src/my contact@support.vue:12:3'."],"expected":[{"kind":"path","text":"src/my contact@support.vue:12:3","start":5,"end":36}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_309() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see `C:\\Users\\Jane Doe\\project\\index.ts:12:7`."],"expected":[{"kind":"path","text":"C:\\Users\\Jane Doe\\project\\index.ts:12:7","start":5,"end":44}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_310() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see `src/my contact@support.vue:12:3`."],"expected":[{"kind":"path","text":"src/my contact@support.vue:12:3","start":5,"end":36}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_311() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src\\my folder\\index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_312() {
    assert_case(r####"{"function":"isTerminalUrl","input":["src\\my"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_313() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["folder\\index.ts"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_314() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"src\\my folder\\index.ts\":12:7: error"],"expected":[{"kind":"path","text":"src\\my folder\\index.ts:12:7","start":1,"end":29}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_315() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["./my folder/index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_316() {
    assert_case(r####"{"function":"isTerminalUrl","input":["./my"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_317() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["folder/index.ts"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_318() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"./my folder/index.ts\":12:7: error"],"expected":[{"kind":"path","text":"./my folder/index.ts:12:7","start":1,"end":27}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_319() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/home/Jane Doe/index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_320() {
    assert_case(r####"{"function":"isTerminalUrl","input":["Doe/index.ts"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_321() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"/home/Jane Doe/index.ts\":12:7: error"],"expected":[{"kind":"path","text":"/home/Jane Doe/index.ts:12:7","start":1,"end":30}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_322() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/home/Jane Doe/file):12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_323() {
    assert_case(r####"{"function":"isTerminalUrl","input":["Doe/file"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_324() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"/home/Jane Doe/file)\":12:7: error"],"expected":[{"kind":"path","text":"/home/Jane Doe/file):12:7","start":1,"end":27}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_325() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["\\\\server\\my share\\index.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_326() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["share\\index.ts"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_327() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"\\\\server\\my share\\index.ts\":12:7: error"],"expected":[{"kind":"path","text":"\\\\server\\my share\\index.ts:12:7","start":1,"end":33}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_328() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error: \"something went wrong\""],"expected":[]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_329() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error: \"something went wrong.\""],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_330() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\Jane Doe\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_331() {
    assert_case(r####"{"function":"isTerminalUrl","input":["C:\\Jane"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_332() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["Doe\\index.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_333() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://example.com/docs"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_334() {
    assert_case(r####"{"function":"isTerminalUrl","input":["src/index.ts"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_335() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["\"C:\\Jane Doe\\index.ts:12\" https://example.com/docs src/index.ts"],"expected":[{"kind":"path","text":"C:\\Jane Doe\\index.ts:12","start":1,"end":24},{"kind":"url","text":"https://example.com/docs","start":26,"end":50},{"kind":"path","text":"src/index.ts","start":51,"end":63}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_336() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["HTTPS://example.com/docs"],"expected":true}"####,
    );
}
#[test]
fn ts_is_terminal_url_337() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["Http://example.com"],"expected":true}"####,
    );
}
#[test]
fn ts_is_terminal_url_338() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/components/main.ts"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_339() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["httpsdocs/readme.md"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_340() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["./src/main.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_341() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["(https://example.com/docs), ./src/main.ts:12."],"expected":[{"kind":"url","text":"https://example.com/docs","start":1,"end":25},{"kind":"path","text":"./src/main.ts:12","start":28,"end":44}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_342() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/żółć.ts:12:7"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_343() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/żółć.ts:12:7 for details"],"expected":[{"kind":"path","text":"src/żółć.ts:12:7","start":4,"end":20}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_344() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["żółć/main.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_345() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see żółć/main.ts:12 for details"],"expected":[{"kind":"path","text":"żółć/main.ts:12","start":4,"end":19}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_346() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["u017C\\u00F3\\u0142\\u0107\\main.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_347() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see \\u017C\\u00F3\\u0142\\u0107\\main.ts:12 for details"],"expected":[{"kind":"path","text":"u017C\\u00F3\\u0142\\u0107\\main.ts:12","start":5,"end":39}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_348() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/café.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_349() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/café.ts:12 for details"],"expected":[{"kind":"path","text":"src/café.ts:12","start":4,"end":19}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_350() {
    assert_case(r####"{"function":"isTerminalUrl","input":["src/𐐀.ts:12"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_351() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/𐐀.ts:12 for details"],"expected":[{"kind":"path","text":"src/𐐀.ts:12","start":4,"end":16}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_352() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/app/[id]/page.tsx:3:10"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_353() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/app/[id]/page.tsx:3:10 for details"],"expected":[{"kind":"path","text":"src/app/[id]/page.tsx:3:10","start":4,"end":30}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_354() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/routes/(app)/+page.svelte:12:4"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_355() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/routes/(app)/+page.svelte:12:4 for details"],"expected":[{"kind":"path","text":"src/routes/(app)/+page.svelte:12:4","start":4,"end":38}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_356() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/routes/(app)/+page@.svelte:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_357() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/routes/(app)/+page@.svelte:12:3 for details"],"expected":[{"kind":"path","text":"src/routes/(app)/+page@.svelte:12:3","start":4,"end":39}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_358() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/contact@support.vue:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_359() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/contact@support.vue:12:3 for details"],"expected":[{"kind":"path","text":"src/contact@support.vue:12:3","start":4,"end":32}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_360() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src\\contact@support.ts:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_361() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src\\contact@support.ts:12:3 for details"],"expected":[{"kind":"path","text":"src\\contact@support.ts:12:3","start":4,"end":31}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_362() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/équipe@home.ts:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_363() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/équipe@home.ts:12:3 for details"],"expected":[{"kind":"path","text":"src/équipe@home.ts:12:3","start":4,"end":27}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_364() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/foo$contact@support.ts:12:3"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_365() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see src/foo$contact@support.ts:12:3 for details"],"expected":[{"kind":"path","text":"src/foo$contact@support.ts:12:3","start":4,"end":35}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_366() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["node_modules/.pnpm/react@18.3.1/node_modules/react/index.js:12:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_367() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see node_modules/.pnpm/react@18.3.1/node_modules/react/index.js:12:5 for details"],"expected":[{"kind":"path","text":"node_modules/.pnpm/react@18.3.1/node_modules/react/index.js:12:5","start":4,"end":68}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_368() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["app/routes/posts.$id.tsx:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_369() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see app/routes/posts.$id.tsx:5 for details"],"expected":[{"kind":"path","text":"app/routes/posts.$id.tsx:5","start":4,"end":30}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_370() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:/Users/someone/project/src/file.ts:42"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_371() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["see C:/Users/someone/project/src/file.ts:42 for details"],"expected":[{"kind":"path","text":"C:/Users/someone/project/src/file.ts:42","start":4,"end":43}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_372() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://example.test/items/foo"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_373() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["GET https://example.test/items/foo:"],"expected":[{"kind":"url","text":"https://example.test/items/foo:","start":4,"end":35}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_374() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["./main.go:10:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_375() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["./main.go:10:5: undefined: x"],"expected":[{"kind":"path","text":"./main.go:10:5","start":0,"end":14}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_376() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/home/dev/app/src/main.c:10:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_377() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["/home/dev/app/src/main.c:10:5: error: expected ';'"],"expected":[{"kind":"path","text":"/home/dev/app/src/main.c:10:5","start":0,"end":29}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_378() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:\\dev\\app\\src\\main.c:10:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_379() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["C:\\dev\\app\\src\\main.c:10:5: error: expected ';'"],"expected":[{"kind":"path","text":"C:\\dev\\app\\src\\main.c:10:5","start":0,"end":26}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_380() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["./out/report.txt"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_381() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["wrote ./out/report.txt:"],"expected":[{"kind":"path","text":"./out/report.txt","start":6,"end":22}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_382() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/api/trpc/post.list,user.me?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%7D%7D"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_383() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":[" GET /api/trpc/post.list,user.me?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%7D%7D 200 in 35ms"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_384() {
    assert_case(r####"{"function":"isTerminalUrl","input":["/api?+debug"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_385() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["GET /api?+debug 200"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_386() {
    assert_case(r####"{"function":"isTerminalUrl","input":["/posts?page=2"],"expected":false}"####);
}
#[test]
fn ts_extract_terminal_links_387() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["Started GET \"/posts?page=2\" for ::1"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_388() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["api/trpc/post.list"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_389() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["proxying api/trpc/post.list?batch=1 to the backend"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_390() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["src/server.ts:12"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_391() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/api/users?id=42"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_392() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/app/bin/server:3:7"],"expected":false}"####,
    );
}
#[test]
fn ts_is_terminal_url_393() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://example.com/api/users?id=42"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_394() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["src/server.ts:12 GET /api/users?id=42 200 /app/bin/server:3:7 https://example.com/api/users?id=42"],"expected":[{"kind":"path","text":"src/server.ts:12","start":0,"end":16},{"kind":"path","text":"/app/bin/server:3:7","start":42,"end":61},{"kind":"url","text":"https://example.com/api/users?id=42","start":62,"end":97}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_395() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://127.0.0.1:6379/0"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_396() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["connected to redis://127.0.0.1:6379/0"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_397() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://localhost:5432/app"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_398() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["connected to postgres://localhost:5432/app"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_399() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["//bucket/reports/today.csv"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_400() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["uploaded s3://bucket/reports/today.csv"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_401() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["e:///Users/jane/app/src/index.mjs:3:7"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_402() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["    at main (file:///Users/jane/app/src/index.mjs:3:7)"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_403() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://localhost:6379/0"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_404() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["+redis://localhost:6379/0"],"expected":[]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_405() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["-redis://localhost:6379/0"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_406() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["acme/widgets.git"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_407() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["git clone git@github.com:acme/widgets.git"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_408() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["group/sub/project.git"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_409() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["remote: git@gitlab.com:group/sub/project.git"],"expected":[]}"####,
    );
}
#[test]
fn ts_is_terminal_url_410() {
    assert_case(r####"{"function":"isTerminalUrl","input":["src/db.ts:4"],"expected":false}"####);
}
#[test]
fn ts_is_terminal_url_411() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["C:/app/src/db.ts:9"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_412() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["src/db.ts:4 redis://localhost:6379/0 C:/app/src/db.ts:9"],"expected":[{"kind":"path","text":"src/db.ts:4","start":0,"end":11},{"kind":"path","text":"C:/app/src/db.ts:9","start":37,"end":55}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_413() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/tmp/report?draft"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_414() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["wrote /tmp/report?draft"],"expected":[{"kind":"path","text":"/tmp/report?draft","start":6,"end":23}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_415() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["s://example.com/path"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_416() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["prefix   https://example.com/path"],"expected":[{"kind":"url","text":"https://example.com/path","start":9,"end":33}]}"####,
    );
}
#[test]
fn ts_extract_terminal_links_417() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["error at \"src\\my folder\\index.ts\":12:7"],"expected":[{"kind":"path","text":"src\\my folder\\index.ts:12:7","start":10,"end":38}]}"####,
    );
}
#[test]
fn ts_is_terminal_url_418() {
    assert_case(
        r####"{"function":"isTerminalUrl","input":["/Users/julius/project/main.c:10:5"],"expected":false}"####,
    );
}
#[test]
fn ts_extract_terminal_links_419() {
    assert_case(
        r####"{"function":"extractTerminalLinks","input":["/Users/julius/project/main.c:10:5: error: expected ';'"],"expected":[{"kind":"path","text":"/Users/julius/project/main.c:10:5","start":0,"end":33}]}"####,
    );
}
#[test]
fn ts_resolve_terminal_link_target_420() {
    assert_case(
        r####"{"function":"resolveTerminalLinkTarget","input":["file:///var/tmp/report.txt","/workspace/project"],"expected":"/var/tmp/report.txt"}"####,
    );
}
#[test]
fn ts_resolve_terminal_link_target_421() {
    assert_case(
        r####"{"function":"resolveTerminalLinkTarget","input":["file://remote.invalid/var/tmp/report.txt","/workspace/project"],"expected":"\\\\remote.invalid\\var\\tmp\\report.txt"}"####,
    );
}
#[test]
fn ts_text_direction_422() {
    assert_case(
        r####"{"function":"textDirection","input":["اكتب دالة تجمع الأرقام"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_423() {
    assert_case(r####"{"function":"textDirection","input":["תקן את הבאג"],"expected":"rtl"}"####);
}
#[test]
fn ts_text_direction_424() {
    assert_case(
        r####"{"function":"textDirection","input":["fix the failing test"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_text_direction_425() {
    assert_case(
        r####"{"function":"textDirection","input":["StrictMode: يعيد الـ render مرتين في التطوير"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_426() {
    assert_case(
        r####"{"function":"textDirection","input":["translate the label مرحبا for the login screen"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_text_direction_427() {
    assert_case(
        r####"{"function":"textDirection","input":["Done. تم إصلاح المشكلة في الملف الرئيسي"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_428() {
    assert_case(r####"{"function":"textDirection","input":["React مكتبة"],"expected":"ltr"}"####);
}
#[test]
fn ts_text_direction_429() {
    assert_case(r####"{"function":"textDirection","input":["مكتبة React"],"expected":"rtl"}"####);
}
#[test]
fn ts_text_direction_430() {
    assert_case(
        r####"{"function":"textDirection","input":["عدّل السطر 42، ثم شغّل الاختبارات (3)"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_431() {
    assert_case(
        r####"{"function":"textDirection","input":["هذا مثال على paragraph يحتوي على نص عربي و English words معًا. Arabic text should flow right-to-left while English words like JavaScript, React, and API should remain left-to-right within the same line. الأرقام 1234567890 ورموز مثل @username و #hashtag و 100$ تظهر في الاتجاه الصحيح. هذه جملة لاختبار RTL rendering في المتصفح أو المحرر."],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_432() {
    assert_case(
        r####"{"function":"textDirection","input":["Run the migration first. ثم شغّل الاختبارات كلها. Then deploy to staging."],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_text_direction_433() {
    assert_case(
        r####"{"function":"textDirection","input":["هذه جملة عربية تتحدث عن e.g. React i.e. TypeScript JavaScript"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_434() {
    assert_case(
        r####"{"function":"textDirection","input":["This long English sentence describes the technology sector in the U.S. هذه جملة عربية. وهذه جملة عربية أخرى."],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_435() {
    assert_case(r####"{"function":"textDirection","input":["42 — 7 ✓"]}"####);
}
#[test]
fn ts_text_direction_436() {
    assert_case(
        r####"{"function":"textDirection","input":["תתקן את זה:\n \nותריץ שוב"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_437() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["תתקן את זה:\n```ts\nexport function useCheckout(props: CheckoutProps) {\n  return props.items.length;\n}\n```\nותריץ שוב"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_438() {
    assert_case(
        r####"{"function":"textDirection","input":["לערוך את   או להשאיר  ?"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_439() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["לערוך את `apps/web/src/chat.tsx` או להשאיר `useMemo`?"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_440() {
    assert_case(r####"{"function":"textDirection","input":["תבדוק   עכשיו"],"expected":"rtl"}"####);
}
#[test]
fn ts_markdown_prose_direction_441() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["תבדוק ``const a = `b c d e` + f`` עכשיו"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_442() {
    assert_case(
        r####"{"function":"textDirection","input":[" \nשלום עולם, זה עובד"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_443() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["````md\none two three four five six\n````\nשלום עולם, זה עובד"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_444() {
    assert_case(
        r####"{"function":"textDirection","input":[" \nזה נכשל אצלי"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_445() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["~~~\nnpm run build --filter web --force\n~~~\nזה נכשל אצלי"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_446() {
    assert_case(
        r####"{"function":"textDirection","input":["שלום, הנה השגיאה:\n "],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_447() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["שלום, הנה השגיאה:\n```\nError: cannot find module react dom client"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_448() {
    assert_case(r####"{"function":"textDirection","input":["ראה   לפני"],"expected":"rtl"}"####);
}
#[test]
fn ts_markdown_prose_direction_449() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["ראה https://github.com/pingdotgg/t3code/pull/24050 לפני"],"expected":"rtl"}"####,
    );
}
#[test]
fn ts_text_direction_450() {
    assert_case(
        r####"{"function":"textDirection","input":["Should I edit   or keep it?"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_451() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["Should I edit `useMemo` or keep it?"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_text_direction_452() {
    assert_case(
        r####"{"function":"textDirection","input":["Please check the שלום label before the release"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_markdown_prose_direction_453() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["Please check the שלום label before the release"],"expected":"ltr"}"####,
    );
}
#[test]
fn ts_text_direction_454() {
    assert_case(r####"{"function":"textDirection","input":[" "]}"####);
}
#[test]
fn ts_markdown_prose_direction_455() {
    assert_case(
        r####"{"function":"markdownProseDirection","input":["```\nconst a = 1;\n```"]}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_456() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"type":"dynamic_tool","toolName":"js","input":{"code":"await sky.get_app_state(app);"}}],"expected":{"toolSurface":"computer","toolSource":{"key":"computer-use","name":"Computer Use","kind":"computer"}}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_457() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"type":"dynamic_tool","toolName":"js","input":{"code":"await sky.get_app_state(app);"},"toolSurface":"browser"}],"expected":{"toolSurface":"browser"}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_458() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"type":"dynamic_tool","toolName":"js","input":{"code":"await fs.readFile(path);"}}],"expected":{}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_459() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"type":"dynamic_tool","toolName":"node_repl.js","title":"js","status":"failed","toolSource":{"key":"mcp:node_repl","name":"node repl","kind":"integration"},"input":{"title":"Restrict the breakpoint to Git actions","code":"await sky.type_text({app:'T3 Code (Nightly)',text:'debug(ga.run)'});"}}],"expected":{"toolSurface":"computer","toolSource":{"key":"computer-use","name":"Computer Use","kind":"computer"}}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_460() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"type":"dynamic_tool","toolName":"node_repl.js","title":"js","status":"failed","toolSource":{"key":"mcp:node_repl","name":"node repl","kind":"integration"},"input":{"code":"await fetch('http://127.0.0.1/json/list');"}}],"expected":{"toolSource":{"key":"mcp:node_repl","name":"node repl","kind":"integration"}}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_461() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"viewedImagePath":" /workspace/reference.webp "}],"expected":{"viewedImagePath":"/workspace/reference.webp"}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_462() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"viewedImagePath":"/workspace/README.md"}],"expected":{}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_463() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"viewedImagePath":"/workspace/a.png\nother"}],"expected":{}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_464() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"viewedImagePath":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.png"}],"expected":{}}"####,
    );
}
#[test]
fn ts_extract_tool_activity_presentation_465() {
    assert_case(
        r####"{"function":"extractToolActivityPresentation","input":[{"data":{"item":{"arguments":{"code":"await sky.click({ app: \"Finder\" })"},"result":{"_meta":{"codex/toolSurface":{"kind":"computerUse","app":{"kind":"displayName","displayName":"Finder"}}}}}}}],"expected":{}}"####,
    );
}
#[test]
fn ts_get_question_answer_text_466() {
    assert_case(
        r####"{"function":"getQuestionAnswerText","input":["See my note: [Review comment: notes.md L2; ref=review-comment_c1]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"review-comment_c1\">\nfile: notes.md\nrange: L2 (2-2)\ncomment:\n  Rename this\ndiff:\n  +line changed\n</context>\n</t3_context>"],"expected":"See my note: [Review comment: notes.md L2]"}"####,
    );
}
#[test]
fn ts_get_question_answer_text_467() {
    assert_case(
        r####"{"function":"getQuestionAnswerText","input":["See my note: [Review comment: notes.md L2; ref=review-comment_c1]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"review-comment_c1\">\nfile: notes.md\nrange: L2 (2-2)\ncomment:\n  Rename this\ndiff:\n  +line changed\n</context>\n</t3_context>",0,["See my note: [Review comment: notes.md L2; ref=review-comment_c1]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"review-comment_c1\">\nfile: notes.md\nrange: L2 (2-2)\ncomment:\n  Rename this\ndiff:\n  +line changed\n</context>\n</t3_context>"]],"expected":"See my note: [Review comment: notes.md L2]"}"####,
    );
}
#[test]
fn ts_get_question_answer_preview_468() {
    assert_case(
        r####"{"function":"getQuestionAnswerPreview","input":[{"requestId":"request-1","answers":{"q1":"See my note: [Review comment: notes.md L2; ref=review-comment_c1]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"review-comment_c1\">\nfile: notes.md\nrange: L2 (2-2)\ncomment:\n  Rename this\ndiff:\n  +line changed\n</context>\n</t3_context>"},"attachmentsByQuestionId":{},"questionTextById":{"q1":"What should change?"}}],"expected":"See my note: [Review comment: notes.md L2]"}"####,
    );
}
#[test]
fn ts_get_question_answer_text_469() {
    assert_case(
        r####"{"function":"getQuestionAnswerText","input":["Use this XML literally: <t3_context version=\"1\"><context kind=\"review-comment\" id=\"example\">keep me</context></t3_context>"],"expected":"Use this XML literally: <t3_context version=\"1\"><context kind=\"review-comment\" id=\"example\">keep me</context></t3_context>"}"####,
    );
}
#[test]
fn ts_with_visible_control_characters_470() {
    assert_case(
        r####"{"function":"withVisibleControlCharacters","input":["vp test run"],"expected":"vp test run"}"####,
    );
}
#[test]
fn ts_command_display_text_471() {
    assert_case(
        r####"{"function":"commandDisplayText","input":["vp test run"],"expected":"vp test run"}"####,
    );
}
#[test]
fn ts_turn_item_detail_revision_472() {
    assert_case(
        r####"{"function":"turnItemDetailRevision","input":[{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"weather.get_weather","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"weather","tool":"get_weather","resourceUri":"ui://weather/dashboard"},"result":{"content":[]}}}],"expected":"2026-09-04T12:00:02.000Z"}"####,
    );
}
#[test]
fn ts_turn_item_detail_revision_473() {
    assert_case(
        r####"{"function":"turnItemDetailRevision","input":[{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"repro.aggregate_rum_events","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"repro","tool":"aggregate_rum_events","resourceUri":"ui://repro/dataviz"},"result":{"content":[{"type":"text","text":"Notice\nview\tcount\n/home\t42"}]}}}],"expected":"2026-09-04T12:00:02.000Z"}"####,
    );
}
#[test]
fn ts_turn_item_detail_revision_474() {
    assert_case(
        r####"{"function":"turnItemDetailRevision","input":[{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"repro.aggregate_rum_events","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"repro","tool":"aggregate_rum_events","resourceUri":"ui://repro/dataviz"},"result":{"content":[{"type":"text","text":"rows"}]}}}],"expected":"2026-09-04T12:00:02.000Z"}"####,
    );
}
#[test]
fn ts_turn_item_detail_revision_475() {
    assert_case(
        r####"{"function":"turnItemDetailRevision","input":[{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"failed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"github.create_pull_request","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"github","tool":"create_pull_request","resourceUri":"ui://github-mcp-server/pr-create"},"result":{"content":[],"isError":true}}}],"expected":"2026-09-04T12:00:02.000Z"}"####,
    );
}
#[test]
fn ts_project_pending_user_input_answer_476() {
    assert_case(
        r####"{"function":"projectPendingUserInputAnswer","input":["Compare [Reference work](t3-context://v1/thread/thread_reference-thread)",[{"version":1,"kind":"thread","contextId":"thread_reference-thread","label":"Reference work","environmentId":"env-answer","threadId":"reference-thread","title":"Reference work"}]],"expected":"Compare [Reference work](t3-thread://v2/env-answer/reference-thread)"}"####,
    );
}
#[test]
fn ts_project_pending_user_input_answer_477() {
    assert_case(
        r####"{"function":"projectPendingUserInputAnswer","input":["See my note: [notes.md L2](t3-context://v1/review-comment/review-comment_file-comment-1)",[{"version":1,"contextId":"review-comment_file-comment-1","kind":"review-comment","label":"notes.md L2","sectionId":"branch","sectionTitle":"Changes","filePath":"notes.md","startIndex":2,"endIndex":2,"rangeLabel":"+2","text":"Rename this to line two updated","diff":"@@ -0,0 +2,1 @@\n+line two changed","fenceLanguage":"diff"}]],"expected":"See my note: [Review comment: notes.md L2; ref=review-comment_file-comment-1]\n\n<t3_context version=\"1\">\n<context kind=\"review-comment\" id=\"review-comment_file-comment-1\">\nfile: notes.md\nrange: +2 (2-2)\nsection: Changes\ncomment:\n  Rename this to line two updated\ndiff:\n  @@ -0,0 +2,1 @@\n  +line two changed\n</context>\n</t3_context>"}"####,
    );
}
#[test]
fn ts_project_pending_user_input_answer_478() {
    assert_case(
        r####"{"function":"projectPendingUserInputAnswer","input":[[" first\t","second"],[{"version":1,"kind":"thread","contextId":"thread_reference-thread","label":"Reference work","environmentId":"env-answer","threadId":"reference-thread","title":"Reference work"}]],"expected":[" first\t","second"]}"####,
    );
}
#[test]
fn ts_project_pending_user_input_answer_479() {
    assert_case(
        r####"{"function":"projectPendingUserInputAnswer","input":["  Edited answer\n",[{"version":1,"kind":"thread","contextId":"thread_reference-thread","label":"Reference work","environmentId":"env-answer","threadId":"reference-thread","title":"Reference work"}]],"expected":"  Edited answer\n"}"####,
    );
}
#[test]
fn ts_thread_contexts_referenced_by_answer_480() {
    assert_case(
        r####"{"function":"threadContextsReferencedByAnswer","input":["Compare [Kept work](t3-context://v1/thread/thread_kept-thread)",[{"version":1,"kind":"thread","contextId":"thread_deleted-thread","label":"Deleted work","environmentId":"env-answer","threadId":"deleted-thread","title":"Deleted work"},{"version":1,"kind":"thread","contextId":"thread_kept-thread","label":"Kept work","environmentId":"env-answer","threadId":"kept-thread","title":"Kept work"}]],"expected":[{"version":1,"kind":"thread","contextId":"thread_kept-thread","label":"Kept work","environmentId":"env-answer","threadId":"kept-thread","title":"Kept work"}]}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_481() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["draft",null],"expected":"draft"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_482() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["draft","   "],"expected":"draft"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_483() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["","also rename the flag "],"expected":"also rename the flag"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_484() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["first half\n","second half"],"expected":"first half\n\nsecond half"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_485() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["","first answer "],"expected":"first answer"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_486() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["first answer","first answer "],"expected":""}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_487() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["draft","first answer"],"expected":"draft\n\nfirst answer"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_488() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["draft\n\nfirst answer","first answer"],"expected":"draft"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_489() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["first answer","second answer"],"expected":"first answer\n\nsecond answer"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_490() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["first answer\n\nsecond answer","first answer",["second answer"]],"expected":"second answer"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_491() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["draft\n\nfirst answer","second answer"],"expected":"draft\n\nfirst answer\n\nsecond answer"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_492() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["draft\n\nfirst answer\n\nsecond answer","first answer",["second answer"]],"expected":"draft\n\nsecond answer"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_493() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["","foo"],"expected":"foo"}"####,
    );
}
#[test]
fn ts_carry_displaced_custom_answer_into_prompt_494() {
    assert_case(
        r####"{"function":"carryDisplacedCustomAnswerIntoPrompt","input":["foo","bar\n\nfoo"],"expected":"foo\n\nbar\n\nfoo"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_495() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["foo\n\nbar\n\nfoo","foo",["bar\n\nfoo"]],"expected":"bar\n\nfoo"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_496() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["first answer, edited","first answer"],"expected":"first answer, edited"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_497() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["my first answer","first answer"],"expected":"my first answer"}"####,
    );
}
#[test]
fn ts_withdraw_displaced_custom_answer_from_prompt_498() {
    assert_case(
        r####"{"function":"withdrawDisplacedCustomAnswerFromPrompt","input":["draft","  "],"expected":"draft"}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_499() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"item-provider-retry","createdAt":"2026-06-20T00:00:00.000Z","runId":"run-provider-retry","tone":"info","itemType":"error","toolLifecycleStatus":"completed","structuredPayload":{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"usage_limit","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}},"projectedItem":{"item":{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"usage_limit","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}},"position":0,"visibility":"local","sourceThreadId":"thread-provider-retry","sourceItemId":"item-provider-retry"},"label":"Provider recovered (2/10 retries)","detail":"Claude API overloaded.","toolData":{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"usage_limit","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_500() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_501() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_502() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_503() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_504() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_505() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_506() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_507() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_508() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}],"expected":false}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_509() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}],"expected":null}"####,
    );
}
#[test]
fn ts_tool_group_action_510() {
    assert_case(
        r####"{"function":"toolGroupAction","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}],"expected":"command"}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_511() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"completed"],"expected":null}"####,
    );
}
#[test]
fn ts_work_entry_viewed_image_path_512() {
    assert_case(
        r####"{"function":"workEntryViewedImagePath","input":[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}],"expected":null}"####,
    );
}
#[test]
fn ts_work_entries_viewed_image_paths_513() {
    assert_case(
        r####"{"function":"workEntriesViewedImagePaths","input":[[{"id":"before","createdAt":"2026-09-04T12:00:01.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":1,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"before","item":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"before","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:01.000Z","completedAt":"2026-09-04T12:00:01.000Z","updatedAt":"2026-09-04T12:00:01.000Z","type":"command_execution","input":"vp test run"}}]],"expected":[]}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_514() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}],"expected":false}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_515() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}],"expected":null}"####,
    );
}
#[test]
fn ts_tool_group_action_516() {
    assert_case(
        r####"{"function":"toolGroupAction","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}],"expected":"command"}"####,
    );
}
#[test]
fn ts_resolve_work_entry_tool_presentation_517() {
    assert_case(
        r####"{"function":"resolveWorkEntryToolPresentation","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"completed"],"expected":null}"####,
    );
}
#[test]
fn ts_work_entry_viewed_image_path_518() {
    assert_case(
        r####"{"function":"workEntryViewedImagePath","input":[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}],"expected":null}"####,
    );
}
#[test]
fn ts_work_entries_viewed_image_paths_519() {
    assert_case(
        r####"{"function":"workEntriesViewedImagePaths","input":[[{"id":"after","createdAt":"2026-09-04T12:00:03.000Z","runId":"render-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"},"projectedItem":{"position":3,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"after","item":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}},"label":"Ran command","command":"vp test run","rawCommand":"vp test run","toolTitle":"Command","toolData":{"id":"after","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:03.000Z","completedAt":"2026-09-04T12:00:03.000Z","updatedAt":"2026-09-04T12:00:03.000Z","type":"command_execution","input":"vp test run"}}]],"expected":[]}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_520() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"render","createdAt":"2026-09-04T12:00:02.000Z","runId":"render-run","tone":"tool","itemType":"dynamic_tool","toolLifecycleStatus":"completed","structuredPayload":{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"weather.get_weather","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"weather","tool":"get_weather","resourceUri":"ui://weather/dashboard"},"result":{"content":[]}}},"projectedItem":{"position":2,"visibility":"local","sourceThreadId":"render-thread","sourceItemId":"render","item":{"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":"weather.get_weather","input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"weather","tool":"get_weather","resourceUri":"ui://weather/dashboard"},"result":{"content":[]}}}},"label":"weather.get_weather","toolTitle":"weather.get_weather","toolData":{"input":{"title":"Chart","height":420},"output":{"t3McpApp":{"attachmentId":"render-thread-app-html","server":"weather","tool":"get_weather","resourceUri":"ui://weather/dashboard"},"result":{"content":[]}}}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_521() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"entry-1","createdAt":"2026-08-27T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","detail":"bash: foo: command not found"}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_522() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"entry-1","createdAt":"2026-08-27T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","detail":"<exited with exit code 2>"}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_failure_523() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolFailure","input":[{"id":"entry-1","createdAt":"2026-08-27T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","command":"rg 'command not found' src/"}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_display_indicates_tool_failure_524() {
    assert_case(
        r####"{"function":"workEntryDisplayIndicatesToolFailure","input":[{"id":"entry-1","createdAt":"2026-08-27T00:00:00.000Z","label":"Ran command","tone":"tool","toolLifecycleStatus":"completed","command":"rg 'command not found' src/"}],"expected":false}"####,
    );
}
#[test]
fn ts_is_latest_run_settled_525() {
    assert_case(
        r####"{"function":"isLatestRunSettled","input":[{"runId":"run-1","status":"completed","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:01:00.000Z"},null],"expected":true}"####,
    );
}
#[test]
fn ts_is_latest_run_settled_526() {
    assert_case(
        r####"{"function":"isLatestRunSettled","input":[{"runId":"run-1","status":"running","startedAt":null,"completedAt":null},{"status":"running","activeRunId":"run-1"}],"expected":false}"####,
    );
}
#[test]
fn ts_is_latest_run_settled_527() {
    assert_case(
        r####"{"function":"isLatestRunSettled","input":[{"runId":"run-1","status":"queued","startedAt":null,"completedAt":null},{"status":"running","activeRunId":"run-active"}],"expected":false}"####,
    );
}
#[test]
fn ts_derive_phase_528() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"preparing","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"connecting"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_529() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"preparing","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":true}"####,
    );
}
#[test]
fn ts_derive_phase_530() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"starting","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"connecting"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_531() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"starting","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":true}"####,
    );
}
#[test]
fn ts_derive_phase_532() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"running","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"running"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_533() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"running","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":true}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_534() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[false,{"status":"running","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":false}"####,
    );
}
#[test]
fn ts_derive_phase_535() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"queued","activeRunId":null,"providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"connecting"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_536() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"queued","activeRunId":null,"providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":false}"####,
    );
}
#[test]
fn ts_derive_phase_537() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"queued","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"connecting"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_538() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"queued","activeRunId":"run-stop-while-starting","providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":true}"####,
    );
}
#[test]
fn ts_derive_phase_539() {
    assert_case(
        r####"{"function":"derivePhase","input":[{"status":"waiting","activeRunId":null,"providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":"running"}"####,
    );
}
#[test]
fn ts_derive_can_interrupt_running_thread_540() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,{"status":"waiting","activeRunId":null,"providerInstanceId":"claude-default","providerName":null,"lastError":null,"updatedAt":"2026-09-29T00:00:00.000Z"}],"expected":true}"####,
    );
}
#[test]
fn ts_derive_phase_541() {
    assert_case(r####"{"function":"derivePhase","input":[null],"expected":"disconnected"}"####);
}
#[test]
fn ts_derive_can_interrupt_running_thread_542() {
    assert_case(
        r####"{"function":"deriveCanInterruptRunningThread","input":[true,null],"expected":false}"####,
    );
}
#[test]
fn ts_provider_error_presentation_543() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"failed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":null,"updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"usage_limit","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}}],"expected":{"label":"Usage limit reached after 2/10 retries","detail":"Claude API overloaded."}}"####,
    );
}
#[test]
fn ts_provider_error_presentation_544() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"usage_limit","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}}],"expected":{"label":"Provider recovered (2/10 retries)","detail":"Claude API overloaded."}}"####,
    );
}
#[test]
fn ts_provider_error_presentation_545() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":null,"updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"provider_error","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}}],"expected":{"label":"Retrying provider (2/10)","detail":"Claude API overloaded. Retrying in 1.5s."}}"####,
    );
}
#[test]
fn ts_provider_error_presentation_546() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"provider_error","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":2,"maxAttempts":10,"retryDelayMs":1500}}],"expected":{"label":"Provider recovered (2/10 retries)","detail":"Claude API overloaded."}}"####,
    );
}
#[test]
fn ts_provider_error_presentation_547() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-provider-retry","threadId":"thread-provider-retry","runId":"run-provider-retry","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"failed","title":"Provider retry","startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"provider_error","message":"Claude API overloaded.","code":"api_error_529","retryable":true},"retry":{"attempt":10,"maxAttempts":10,"retryDelayMs":1500}}],"expected":{"label":"Provider error after 10/10 retries","detail":"Claude API overloaded."}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_548() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"message-turn-start","kind":"message","createdAt":"2026-06-20T00:00:00.000Z","message":{"id":"message-turn-start","role":"user","text":"Start","runId":"run-steered","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z"}},{"id":"message-steer","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"message-steer","role":"user","text":"Steer","runId":"run-steered","inputIntent":"steer","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"message-assistant","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"message-assistant","role":"assistant","text":"Done","runId":"run-steered","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"threadStartCheckpointAvailable":true,"checkpoints":[{"runId":"run-steered","checkpointTurnCount":1,"checkpointRef":"checkpoint-run-1","status":"ready","files":[],"assistantMessageId":"message-assistant","completedAt":"2026-06-20T00:00:03.000Z"}]}],"expected":{"message-turn-start":0}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_549() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"user-4","kind":"message","createdAt":"2026-06-20T00:00:04.000Z","message":{"id":"user-4","role":"user","text":"user","runId":"run-4","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:04.000Z","updatedAt":"2026-06-20T00:00:04.000Z"}},{"id":"assistant-4","kind":"message","createdAt":"2026-06-20T00:00:04.000Z","message":{"id":"assistant-4","role":"assistant","text":"assistant","runId":"run-4","streaming":false,"createdAt":"2026-06-20T00:00:04.000Z","updatedAt":"2026-06-20T00:00:04.000Z"}},{"id":"user-5","kind":"message","createdAt":"2026-06-20T00:00:05.000Z","message":{"id":"user-5","role":"user","text":"user","runId":"run-5","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:05.000Z","updatedAt":"2026-06-20T00:00:05.000Z"}},{"id":"assistant-5","kind":"message","createdAt":"2026-06-20T00:00:05.000Z","message":{"id":"assistant-5","role":"assistant","text":"assistant","runId":"run-5","streaming":false,"createdAt":"2026-06-20T00:00:05.000Z","updatedAt":"2026-06-20T00:00:05.000Z"}}],"checkpoints":[{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-3","checkpointTurnCount":3,"checkpointRef":"checkpoint-3","status":"stale","files":[],"assistantMessageId":"assistant-3","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-4","checkpointTurnCount":4,"checkpointRef":"checkpoint-4","status":"ready","files":[],"assistantMessageId":"assistant-4","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":null}],"expected":{"user-2":0,"user-4":2,"user-5":4}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_550() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"stale","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":null}],"expected":{"user-2":0}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_551() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"missing","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"missing","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":null}],"expected":{"user-1":0,"user-2":1}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_552() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"wake-item","kind":"work","createdAt":"2026-09-04T00:00:00.000Z","entry":{"id":"wake-item","createdAt":"2026-09-04T00:00:00.000Z","runId":"run-2","label":"Delegated task finished","tone":"info","itemType":"notification","structuredPayload":{"id":"wake-item","threadId":"streaming-thread","runId":"run-2","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","type":"notification","messageId":"history-user","text":"Inspect","attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web","source":{"kind":"delegated_task","taskIds":["task-1"]},"outcome":"unknown","summary":"Delegated task finished"},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"history-user-item","item":{"id":"wake-item","threadId":"streaming-thread","runId":"run-2","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","type":"notification","messageId":"history-user","text":"Inspect","attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web","source":{"kind":"delegated_task","taskIds":["task-1"]},"outcome":"unknown","summary":"Delegated task finished"}}}},{"id":"inherited-wake","kind":"work","createdAt":"2026-09-04T00:00:00.000Z","entry":{"id":"inherited-wake","createdAt":"2026-09-04T00:00:00.000Z","runId":"run-7","label":"Delegated task finished","tone":"info","itemType":"notification","structuredPayload":{"id":"inherited-wake","threadId":"parent-thread","runId":"run-7","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","type":"notification","messageId":"history-user","text":"Inspect","attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web","source":{"kind":"delegated_task","taskIds":["task-1"]},"outcome":"unknown","summary":"Delegated task finished"},"projectedItem":{"position":0,"visibility":"inherited","sourceThreadId":"parent-thread","sourceItemId":"inherited-wake","item":{"id":"inherited-wake","threadId":"parent-thread","runId":"run-7","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","type":"notification","messageId":"history-user","text":"Inspect","attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web","source":{"kind":"delegated_task","taskIds":["task-1"]},"outcome":"unknown","summary":"Delegated task finished"}}}},{"id":"background-work","kind":"work","createdAt":"2026-06-20T00:00:02.000Z","entry":{"id":"background-work","createdAt":"2026-06-20T00:00:02.000Z","runId":"run-8","itemType":"command_execution","label":"Background work","tone":"tool"}},{"id":"user-3","kind":"message","createdAt":"2026-06-20T00:00:03.000Z","message":{"id":"user-3","role":"user","text":"user","runId":"run-3","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:03.000Z","updatedAt":"2026-06-20T00:00:03.000Z"}},{"id":"assistant-3","kind":"message","createdAt":"2026-06-20T00:00:03.000Z","message":{"id":"assistant-3","role":"assistant","text":"assistant","runId":"run-3","streaming":false,"createdAt":"2026-06-20T00:00:03.000Z","updatedAt":"2026-06-20T00:00:03.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"ready","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-3","checkpointTurnCount":3,"checkpointRef":"checkpoint-3","status":"ready","files":[],"assistantMessageId":"assistant-3","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-7","checkpointTurnCount":7,"checkpointRef":"checkpoint-7","status":"ready","files":[],"assistantMessageId":"assistant-7","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-8","checkpointTurnCount":8,"checkpointRef":"checkpoint-8","status":"ready","files":[],"assistantMessageId":"assistant-8","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true}],"expected":{"user-1":0,"user-3":2}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_553() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":null}],"expected":{"user-1":0}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_554() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"stale","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":null}],"expected":{"user-1":0}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_555() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-4","kind":"message","createdAt":"2026-06-20T00:00:04.000Z","message":{"id":"user-4","role":"user","text":"user","runId":"run-4","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:04.000Z","updatedAt":"2026-06-20T00:00:04.000Z"}},{"id":"assistant-4","kind":"message","createdAt":"2026-06-20T00:00:04.000Z","message":{"id":"assistant-4","role":"assistant","text":"assistant","runId":"run-4","streaming":false,"createdAt":"2026-06-20T00:00:04.000Z","updatedAt":"2026-06-20T00:00:04.000Z"}},{"id":"user-5","kind":"message","createdAt":"2026-06-20T00:00:05.000Z","message":{"id":"user-5","role":"user","text":"user","runId":"run-5","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:05.000Z","updatedAt":"2026-06-20T00:00:05.000Z"}},{"id":"assistant-5","kind":"message","createdAt":"2026-06-20T00:00:05.000Z","message":{"id":"assistant-5","role":"assistant","text":"assistant","runId":"run-5","streaming":false,"createdAt":"2026-06-20T00:00:05.000Z","updatedAt":"2026-06-20T00:00:05.000Z"}}],"checkpoints":[{"runId":"run-4","checkpointTurnCount":4,"checkpointRef":"checkpoint-4","status":"ready","files":[],"assistantMessageId":"assistant-4","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-5","checkpointTurnCount":5,"checkpointRef":"checkpoint-5","status":"ready","files":[],"assistantMessageId":"assistant-5","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":false,"unsettledRunId":null}],"expected":{"user-5":4}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_556() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"ready","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"unsettledRunId":"run-2"}],"expected":{"user-1":0}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_557() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"user-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"user-1","role":"user","text":"user","runId":"run-1","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"assistant-1","kind":"message","createdAt":"2026-06-20T00:00:01.000Z","message":{"id":"assistant-1","role":"assistant","text":"assistant","runId":"run-1","streaming":false,"createdAt":"2026-06-20T00:00:01.000Z","updatedAt":"2026-06-20T00:00:01.000Z"}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"user-3","kind":"message","createdAt":"2026-06-20T00:00:03.000Z","message":{"id":"user-3","role":"user","text":"user","runId":"run-3","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:03.000Z","updatedAt":"2026-06-20T00:00:03.000Z"}},{"id":"assistant-3","kind":"message","createdAt":"2026-06-20T00:00:03.000Z","message":{"id":"assistant-3","role":"assistant","text":"assistant","runId":"run-3","streaming":false,"createdAt":"2026-06-20T00:00:03.000Z","updatedAt":"2026-06-20T00:00:03.000Z"}}],"checkpoints":[{"runId":"run-1","checkpointTurnCount":1,"checkpointRef":"checkpoint-1","status":"ready","files":[],"assistantMessageId":"assistant-1","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-3","checkpointTurnCount":3,"checkpointRef":"checkpoint-3","status":"ready","files":[],"assistantMessageId":"assistant-3","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true,"availableRevertTurnCounts":[0,1]}],"expected":{"user-1":0,"user-2":1}}"####,
    );
}
#[test]
fn ts_derive_revert_turn_count_by_user_message_id_558() {
    assert_case(
        r####"{"function":"deriveRevertTurnCountByUserMessageId","input":[{"timelineEntries":[{"id":"history-user","kind":"message","createdAt":"2026-09-04T00:00:00.000Z","message":{"id":"history-user","role":"user","text":"Inspect","runId":"history-run","streaming":false,"createdBy":"user","creationSource":"web","createdAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","inputIntent":"turn_start"},"projectedItem":{"position":0,"visibility":"inherited","sourceThreadId":"streaming-thread","sourceItemId":"history-user-item","item":{"id":"history-user-item","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","type":"user_message","messageId":"history-user","text":"Inspect","attachments":[],"inputIntent":"turn_start","createdBy":"user","creationSource":"web"}}},{"id":"history-work","kind":"work","createdAt":"2026-09-04T00:00:01.000Z","entry":{"id":"history-work","createdAt":"2026-09-04T00:00:01.000Z","runId":"history-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"failed","structuredPayload":{"id":"history-work","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"failed","title":null,"startedAt":"2026-09-04T00:00:01.000Z","completedAt":"2026-09-04T00:00:01.000Z","updatedAt":"2026-09-04T00:00:01.000Z","type":"command_execution","input":"vp test","output":"Test failed","exitCode":1},"projectedItem":{"position":1,"visibility":"inherited","sourceThreadId":"streaming-thread","sourceItemId":"history-work","item":{"id":"history-work","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"failed","title":null,"startedAt":"2026-09-04T00:00:01.000Z","completedAt":"2026-09-04T00:00:01.000Z","updatedAt":"2026-09-04T00:00:01.000Z","type":"command_execution","input":"vp test","output":"Test failed","exitCode":1}},"label":"Ran command","command":"vp test","rawCommand":"vp test","toolTitle":"Command","toolData":{"id":"history-work","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"failed","title":null,"startedAt":"2026-09-04T00:00:01.000Z","completedAt":"2026-09-04T00:00:01.000Z","updatedAt":"2026-09-04T00:00:01.000Z","type":"command_execution","input":"vp test","output":"Test failed","exitCode":1}}},{"id":"history-assistant","kind":"message","createdAt":"2026-09-04T00:00:03.000Z","message":{"id":"history-assistant","role":"assistant","text":"Done","runId":"history-run","streaming":false,"createdAt":"2026-09-04T00:00:03.000Z","updatedAt":"2026-09-04T00:00:04.000Z"},"projectedItem":{"position":2,"visibility":"inherited","sourceThreadId":"streaming-thread","sourceItemId":"history-assistant-item","item":{"id":"history-assistant-item","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":3,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:03.000Z","completedAt":"2026-09-04T00:00:03.000Z","updatedAt":"2026-09-04T00:00:04.000Z","type":"assistant_message","messageId":"history-assistant","text":"Done","streaming":false}}},{"id":"user-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"user-2","role":"user","text":"user","runId":"run-2","inputIntent":"turn_start","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}},{"id":"assistant-2","kind":"message","createdAt":"2026-06-20T00:00:02.000Z","message":{"id":"assistant-2","role":"assistant","text":"assistant","runId":"run-2","streaming":false,"createdAt":"2026-06-20T00:00:02.000Z","updatedAt":"2026-06-20T00:00:02.000Z"}}],"checkpoints":[{"runId":"history-run","checkpointTurnCount":7,"checkpointRef":"checkpoint-7","status":"ready","files":[],"assistantMessageId":"assistant-7","completedAt":"2026-06-20T00:00:09.000Z"},{"runId":"run-2","checkpointTurnCount":2,"checkpointRef":"checkpoint-2","status":"ready","files":[],"assistantMessageId":"assistant-2","completedAt":"2026-06-20T00:00:09.000Z"}],"threadStartCheckpointAvailable":true}],"expected":{"user-2":0}}"####,
    );
}
#[test]
fn ts_provider_error_presentation_559() {
    assert_case(
        r####"{"function":"providerErrorPresentation","input":[{"id":"item-error","threadId":"thread-visible","runId":"run-visible","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":5,"status":"failed","title":null,"startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","type":"error","failure":{"class":"validation_error","message":"Invalid reasoning effort.","code":"invalid_request","retryable":false}}],"expected":{"label":"Provider error","detail":"Invalid reasoning effort."}}"####,
    );
}
#[test]
fn ts_work_log_entry_is_tool_like_560() {
    assert_case(
        r####"{"function":"workLogEntryIsToolLike","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_success_561() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolSuccess","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","outputIndicatesFailure":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_log_entry_is_tool_like_562() {
    assert_case(
        r####"{"function":"workLogEntryIsToolLike","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_success_563() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolSuccess","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","exitCode":2}}],"expected":false}"####,
    );
}
#[test]
fn ts_work_log_entry_is_tool_like_564() {
    assert_case(
        r####"{"function":"workLogEntryIsToolLike","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}}],"expected":true}"####,
    );
}
#[test]
fn ts_work_entry_indicates_tool_success_565() {
    assert_case(
        r####"{"function":"workEntryIndicatesToolSuccess","input":[{"id":"native-item","createdAt":"2026-09-04T12:00:00.000Z","runId":"native-run","tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed","structuredPayload":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"},"projectedItem":{"position":0,"visibility":"local","sourceThreadId":"native-thread","sourceItemId":"native-item","item":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}},"label":"Ran command","command":"foo","rawCommand":"foo","toolTitle":"Command","toolData":{"id":"native-item","threadId":"native-thread","runId":"native-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":"2026-09-04T12:00:00.000Z","completedAt":"2026-09-04T12:00:00.000Z","updatedAt":"2026-09-04T12:00:00.000Z","type":"command_execution","input":"foo","output":"bash: foo: command not found"}}],"expected":false}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_566() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true,"attachments":[{"type":"image","id":"stream-image","name":"image.png","mimeType":"image/png","sizeBytes":42}]}},{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:08.000Z","type":"assistant_message","messageId":"live-assistant","text":"Next token","streaming":true,"attachments":[{"type":"image","id":"stream-image","name":"image.png","mimeType":"image/png","sizeBytes":42}]}}],"expected":true}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_567() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}},{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"completed","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":false}}],"expected":false}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_568() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}},{"position":5,"visibility":"local","sourceThreadId":"inherited-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_569() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}},{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_570() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}},{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true,"attachments":[{"type":"image","id":"new-image","name":"new.png","mimeType":"image/png","sizeBytes":42}]}}],"expected":false}"####,
    );
}
#[test]
fn ts_is_streaming_turn_item_text_update_571() {
    assert_case(
        r####"{"function":"isStreamingTurnItemTextUpdate","input":[{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"live-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}},{"position":5,"visibility":"local","sourceThreadId":"streaming-thread","sourceItemId":"live-assistant-item","item":{"id":"live-assistant-item","threadId":"streaming-thread","runId":"history-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":null,"startedAt":"2026-09-04T00:00:07.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:07.000Z","type":"assistant_message","messageId":"live-assistant","text":"Partial","streaming":true}}],"expected":false}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_572() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["thread-send",[{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread","message":"{\"threadId\":\"fake\"}"}},"output":{"messageId":"message-1","threadId":"actual-thread"},"outcome":"completed"},{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread","message":"{\"threadId\":\"fake\"}"}},"output":{"structuredContent":{"messageId":"message-1","threadId":"actual-thread"}},"outcome":"completed"},{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread","message":"{\"threadId\":\"fake\"}"}},"output":[{"type":"text","text":"{\"messageId\":\"message-1\",\"threadId\":\"actual-thread\"}"}],"outcome":"completed"},{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread","message":"{\"threadId\":\"fake\"}"}},"output":{"content":[{"text":{"text":"{\"messageId\":\"message-1\",\"threadId\":\"actual-thread\"}"}}],"isError":false},"outcome":"completed"},{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread","message":"{\"threadId\":\"fake\"}"}},"output":"{\"messageId\":\"message-1\",\"threadId\":\"actual-thread\"}","outcome":"completed"}]],"expected":{"label":"Sent 1 message to 1 thread","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_573() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["thread-send",[{"input":{"toolName":"t3_thread_send","args":{"threadId":"input-thread"}},"outcome":"completed"},{"input":{"threadId":"input-thread"},"outcome":"completed"}]],"expected":{"label":"Sent 2 messages to 1 thread","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_574() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["link-pr",[{"input":{"urls":["https://github.com/o/r/pull/1","https://github.com/o/r/pull/1","not-a-pr"]},"output":{"linkedCount":1},"outcome":"completed"},{"input":{"number":3},"outcome":"completed"}]],"expected":{"label":"Linked 2 pull requests","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_575() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["link-pr",[{"input":{"urls":["https://github.com/o/r/pull/1","https://github.com/o/r/pull/1","not-a-pr"]},"output":{},"outcome":"completed"}]],"expected":{"label":"Linked pull requests","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_576() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["unlink-pr",[{"input":{"states":["merged"]},"output":{"unlinkedCount":2},"outcome":"completed"},{"input":{"number":3},"outcome":"completed"}]],"expected":{"label":"Unlinked 3 pull requests","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_577() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["unlink-pr",[{"input":{"states":["merged"]},"output":{"isError":false},"outcome":"completed"}]],"expected":{"label":"Unlinked pull requests","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_578() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["thread-send",[{"input":{"threadId":"success"},"output":{"messageId":"ok","threadId":"success"},"outcome":"completed"},{"input":{"threadId":"failed"},"output":{"isError":true,"structuredContent":{"threadId":"failed"}},"outcome":"completed"},{"input":{"threadId":"failed"},"output":[{"type":"text","text":"{\"_tag\":\"OrchestratorMcpFailure\"}"}],"outcome":"completed"},{"input":{"threadId":"cancelled"},"outcome":"unfinished"}]],"expected":{"label":"Sent 1 message to 1 thread","failedCount":2}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_579() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["thread-send",[{"input":{"threadId":"failed"},"output":{"isError":true,"structuredContent":{"threadId":"failed"}},"outcome":"completed"}]],"expected":{"label":"Tried to send 1 message to 1 thread","failedCount":1}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_580() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["delegate",[{"input":{},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"},{"input":{},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"}]],"expected":{"label":"Delegated 1 task","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_581() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["task-status",[{"input":{"taskId":"task-1"},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"},{"input":{"taskId":"task-1"},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"},{"input":{"taskId":"task-1"},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"},{"input":{"taskId":"task-1"},"output":{"taskId":"task-1","status":"failed","summary":"command not found"},"outcome":"completed"}]],"expected":{"label":"Checked task status 4 times","failedCount":0}}"####,
    );
}
#[test]
fn ts_summarize_t3_tool_calls_582() {
    assert_case(
        r####"{"function":"summarizeT3ToolCalls","input":["thread-wait",[{"input":{"threadId":"thread-1"},"output":{"threadId":"thread-1","timedOut":true},"outcome":"completed"}]],"expected":{"label":"Waited on 1 thread","failedCount":0}}"####,
    );
}
#[test]
fn ts_derive_pending_user_input_progress_583() {
    assert_case(
        r####"{"function":"derivePendingUserInputProgress","input":[[{"id":"scope","header":"Scope","question":"What should the plan target first?","options":[{"label":"Orchestration-first","description":"Focus on orchestration first"}],"multiSelect":false,"initialAnswer":"  Proposed message\n"}],{"scope":{"customAnswer":""}},0],"expected":{"questionIndex":0,"activeQuestion":{"id":"scope","header":"Scope","question":"What should the plan target first?","options":[{"label":"Orchestration-first","description":"Focus on orchestration first"}],"multiSelect":false,"initialAnswer":"  Proposed message\n"},"activeDraft":{"customAnswer":""},"selectedOptionValues":[],"customAnswer":"","resolvedAnswer":"","usingCustomAnswer":false,"answeredQuestionCount":1,"isLastQuestion":true,"isComplete":true,"canAdvance":true}}"####,
    );
}
#[test]
fn ts_derive_pending_user_input_progress_584() {
    assert_case(
        r####"{"function":"derivePendingUserInputProgress","input":[[{"id":"scope","header":"Scope","question":"What should the plan target first?","options":[{"label":"Orchestration-first","description":"Focus on orchestration first"}],"multiSelect":false},{"id":"compat","header":"Compat","question":"How strict should compatibility be?","options":[{"label":"Keep current envelope","description":"Preserve current wire format"}],"multiSelect":false}],{"scope":{"selectedOptionValues":["Orchestration-first"]}},0],"expected":{"questionIndex":0,"activeQuestion":{"id":"scope","header":"Scope","question":"What should the plan target first?","options":[{"label":"Orchestration-first","description":"Focus on orchestration first"}],"multiSelect":false},"activeDraft":{"selectedOptionValues":["Orchestration-first"]},"selectedOptionValues":["Orchestration-first"],"customAnswer":"","resolvedAnswer":"Orchestration-first","usingCustomAnswer":false,"answeredQuestionCount":1,"isLastQuestion":false,"isComplete":false,"canAdvance":true}}"####,
    );
}
