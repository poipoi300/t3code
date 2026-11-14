use serde::Deserialize;
use serde_json::Value;
use t3_textgen::{
    audio::*, dictionary::*, generation::*, prompts::*, speech::*, text::*, title_context::*,
};

#[test]
fn prompts_and_speech_corrections_match_executed_typescript() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/parity.json")).expect("fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let input = case["input"].clone();
        let prompt = match case["kind"].as_str().expect("kind") {
            "commit" => commit_prompt(&serde_json::from_value(input).expect("commit")),
            "pr" => pr_prompt(&serde_json::from_value(input).expect("pr")),
            "branch" => branch_prompt(&serde_json::from_value(input).expect("branch")),
            "threadTitle" => thread_title_prompt(&serde_json::from_value(input).expect("thread")),
            "review" => review_prompt(&serde_json::from_value(input).expect("review")),
            "voice" => voice_prompt(
                input["instructions"].as_str().expect("instructions"),
                input["transcript"].as_str().expect("transcript"),
                input["draft"].as_str().expect("draft"),
                input["start"].as_u64().expect("start") as usize,
                input["end"].as_u64().expect("end") as usize,
                input["cue"].as_str().expect("cue"),
            ),
            _ => panic!("unknown fixture kind"),
        };
        assert_eq!(
            prompt,
            case["prompt"].as_str().expect("prompt"),
            "prompt fixture {index}"
        );
    }
    #[derive(Deserialize)]
    struct Correction {
        words: Vec<String>,
        entries: Vec<t3_contracts::speech::SpeechCustomWord>,
        text: String,
        expected: String,
    }
    for case in fixture["dictionaryCases"].as_array().expect("dictionary") {
        let case: Correction = serde_json::from_value(case.clone()).expect("correction");
        assert_eq!(
            TextCorrector::new(&case.words, &case.entries).apply(&case.text),
            case.expected,
            "dictionary input {}",
            case.text
        );
    }
    for case in fixture["fillerCases"].as_array().expect("fillers") {
        assert_eq!(
            remove_filler_words(
                case["text"].as_str().expect("text"),
                case["language"].as_str(),
                case["cue"].as_str()
            ),
            case["expected"].as_str().expect("expected")
        );
    }
}

#[test]
fn reply_sanitizers_match_executed_typescript() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/parity.json")).expect("fixture");
    let static_naming = BranchNaming::Static {
        prefix: " Team/\u{fc} ser//x\u{feff}".into(),
    };
    let custom = BranchNaming::Custom {
        instructions: String::new(),
    };
    for case in fixture["sanitizerCases"].as_array().expect("sanitizers") {
        let raw = case["raw"].as_str().expect("raw");
        let actual = serde_json::json!({
            "raw": raw,
            "commit": sanitize_commit_subject(raw),
            "pr": sanitize_pr_title(raw),
            "title": sanitize_thread_title(raw),
            "fragment": sanitize_branch(raw),
            "feature": sanitize_feature_branch(raw),
            "semantic": format_generated_branch(raw, None).expect("semantic"),
            "static": format_generated_branch(raw, Some(&static_naming)).expect("static"),
            "custom": format_generated_branch(raw, Some(&custom)).expect("custom"),
            "trimmed": t3_contracts::wire::trim_js(raw),
            "json": extract_json_object(raw),
        });
        assert_eq!(&actual, case, "sanitizers of {raw:?}");
    }
}

#[test]
fn custom_branch_names_git_cannot_store_are_refused_before_leaving_the_crate() {
    let custom = BranchNaming::Custom {
        instructions: String::new(),
    };
    let input = GenerationInput::Branch(BranchInput {
        naming: Some(custom),
        ..Default::default()
    });
    let decode = |branch: &str| match decode_output(
        &input,
        &serde_json::json!({ "branch": branch }).to_string(),
    )? {
        Generated::Branch { branch } => Ok(branch),
        _ => Err(GenerationError::InvalidOutput),
    };
    assert_eq!(
        decode(" team/ticket-12-fix-login\u{feff}"),
        Ok("team/ticket-12-fix-login".into())
    );
    assert_eq!(decode(&"a".repeat(255)), Ok("a".repeat(255)));
    // Behind the rename's `--`, git's ref check refuses dashes, `..`, control characters, `@{`
    // and `.lock`; it has no length limit, so loose-ref storage would decide.
    for refused in ["a".repeat(256), format!("team/{}", "\u{e9}".repeat(126))] {
        assert_eq!(
            decode(&refused),
            Err(GenerationError::InvalidOutput),
            "{refused}"
        );
    }
}

#[tokio::test]
async fn prompt_extractor_verifies_the_current_specification() {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = std::env::var_os("T3_TEXTGEN_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate_dir.join("../../.."));
    let script = crate_dir.join("tools/extract-prompts.mjs");
    let mut options = t3_process::ProcessOptions::new("node");
    options.args = vec![
        script.into_os_string(),
        "--check".into(),
        "--repo".into(),
        repo.clone().into_os_string(),
    ];
    let output = t3_process::run(options)
        .await
        .expect("TypeScript extraction succeeds");
    assert_eq!(output.code, Some(0));
    assert_eq!(
        std::fs::read(repo.join("apps/server/src/speech/catalog.json")).expect("source catalog"),
        include_bytes!("../fixtures/catalog.json")
    );
}

#[test]
fn unicode_truncation_and_sanitizers_port_the_typescript_regressions() {
    assert_eq!(
        limit_section(&format!("{}🚀 more", "a".repeat(9)), 10),
        format!("{}\n\n[truncated]", "a".repeat(9))
    );
    for (n, ch) in [(71, "🚀"), (71, "e\u{301}"), (65, "👨‍👩‍👧‍👦")] {
        let prefix = "a".repeat(n);
        assert_eq!(
            sanitize_commit_subject(&format!("{prefix}{ch} more")),
            prefix
        );
    }
    for (n, ch, kept) in [(116, "🚀", ""), (115, "🚀", "🚀"), (116, "界", "界")] {
        let prefix = "a".repeat(n);
        assert_eq!(
            sanitize_thread_title(&format!("{prefix}{ch} more")),
            format!("{prefix}{kept}...")
        );
    }
    assert_eq!(
        limit_title_message(
            &format!("{}🚀{}🚀{}", "a".repeat(9), "b".repeat(20), "c".repeat(9)),
            41
        ),
        format!("{}\n[Content truncated]\n{}", "a".repeat(9), "c".repeat(9))
    );
    assert_eq!(
        sanitize_thread_title("{\"title\":\"A much better title\"}"),
        "A much better title"
    );
    assert_eq!(
        sanitize_thread_title(" `  Pairing   failure `\nexplanation"),
        "Pairing failure"
    );
    assert_eq!(sanitize_commit_subject(" . . "), ". ".trim());
    assert_eq!(
        sanitize_branch(" 'Féature/Add Search!!' "),
        "feature/add-search"
    );
    assert_eq!(
        extract_json_object("preface ```json\n{\"title\":\"a } brace\"}\n```"),
        "{\"title\":\"a } brace\"}"
    );
    let input = GenerationInput::ThreadTitle(ThreadTitleInput::default());
    assert_eq!(
        decode_output(&input, "{\"title\":\"title\"}"),
        Err(GenerationError::PlaceholderTitle)
    );
    assert!(
        matches!(decode_output(&input,"{\"title\":\"\"}"),Ok(Generated::ThreadTitle{title,..})if title=="New thread")
    );
    assert_eq!(
        decode_output(&input, "I cannot help"),
        Err(GenerationError::InvalidOutput)
    );
    assert_eq!(
        decode_output(&input, "{\"title\":\"real\",\"needsRefinement\":\"false\"}"),
        Err(GenerationError::InvalidOutput)
    );
}

#[test]
fn title_context_keeps_user_scope_and_attachment_bootstraps() {
    let message = |role, text: String| TitleMessage {
        role,
        text,
        attachments: Vec::new(),
    };
    let context = format_context(&[
        message(TitleRole::System, "Private instructions".into()),
        message(TitleRole::User, "Review QR sharing".into()),
        message(TitleRole::Assistant, "Old findings. ".repeat(2000)),
        message(
            TitleRole::User,
            "Focus on pairing expiry instead. Keep remote access working.".into(),
        ),
        message(
            TitleRole::Assistant,
            "Implementation details. ".repeat(2000),
        ),
        message(TitleRole::User, "Merge it when green.".into()),
    ]);
    assert!(utf16_len(&context.message) <= 8000);
    for text in [
        "USER:\nReview QR sharing",
        "USER:\nFocus on pairing expiry instead. Keep remote access working.",
        "USER:\nMerge it when green.",
        "ASSISTANT:\nImplementation details.",
    ] {
        assert!(context.message.contains(text));
    }
    assert!(!context.message.contains("Private instructions"));
    for prefix in ["", "Ultrathink:\n"] {
        let image=serde_json::from_value(serde_json::json!({"type":"image","id":"diagram","name":"diagram.png","mimeType":"image/png","sizeBytes":10})).expect("image");
        let context = format_context(&[TitleMessage {
            role: TitleRole::User,
            text: format!(
                "{prefix}[User attached one or more files without additional text. Respond using the conversation context and the attached files.]"
            ),
            attachments: vec![image],
        }]);
        assert_eq!(context.message, "USER:\n[Attachments: diagram.png]");
        assert_eq!(context.attachments.len(), 1);
    }
}

#[test]
fn audio_preserves_preroll_onset_tails_and_short_final_frames() {
    for native_streaming in [false, true] {
        let mut audio = SpeechAudio::new(AudioConfig {
            native_streaming,
            ..Default::default()
        })
        .expect("audio");
        let frame = 512;
        let pcm = (0..100 * frame)
            .map(|i| (i / frame + 1) as f32 / 1000.0)
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        for i in 0..100 {
            audio
                .append(&pcm[i * frame..(i + 1) * frame])
                .expect("append");
            audio
                .score(i, &[if (20..24).contains(&i) { 0.9 } else { 0.0 }])
                .expect("score");
            output.extend(audio.take(false).into_iter().flatten());
        }
        output.extend(audio.take(true).into_iter().flatten());
        assert_eq!(
            output,
            pcm[6 * frame..(24 + if native_streaming { 52 } else { 15 }) * frame]
        );
    }
    let mut short = SpeechAudio::new(AudioConfig::default()).expect("audio");
    short.append(&[0.2, 0.1]).expect("append");
    short.score(0, &[0.8]).expect("score");
    assert_eq!(short.take(true), vec![vec![0.2, 0.1]]);
    let mut silent = SpeechAudio::new(AudioConfig::default()).expect("audio");
    silent.append(&vec![0.0; 512 * 4 + 7]).expect("append");
    silent.score(0, &[0.0; 5]).expect("score");
    assert!(silent.take(true).is_empty());
}

#[test]
fn offline_audio_cuts_at_pauses_and_respects_engine_limits() {
    let frame = 512;
    let samples = (100 * 16000 / frame) * frame;
    let pcm = (0..samples)
        .map(|i| (i / frame + 1) as f32 / 10000.0)
        .collect::<Vec<_>>();
    let mut audio = SpeechAudio::new(AudioConfig {
        filter_silence: false,
        ..Default::default()
    })
    .expect("audio");
    let mut chunks = Vec::new();
    for start in (0..samples).step_by(frame * 7) {
        let end = (start + frame * 7).min(samples);
        audio.append(&pcm[start..end]).expect("append");
        let scores = (start / frame..end / frame)
            .map(|i| {
                let t = (i * frame) as f32 / 16000.0;
                if (28.0..29.0).contains(&t) || (55.0..56.0).contains(&t) {
                    0.0
                } else {
                    1.0
                }
            })
            .collect::<Vec<_>>();
        audio.score(start / frame, &scores).expect("score");
        chunks.extend(audio.take(false));
    }
    assert!(!chunks.is_empty());
    assert!((chunks[0].len() as f64 / 16000.0 - 28.5).abs() < 0.1);
    chunks.extend(audio.take(true));
    assert!(chunks.iter().all(|c| c.len() <= 40 * 16000));
    assert_eq!(chunks.into_iter().flatten().collect::<Vec<_>>(), pcm);
    for config in [
        AudioConfig {
            filter_silence: false,
            max_audio_ms: 4000,
            ..Default::default()
        },
        AudioConfig {
            filter_silence: false,
            long_form: true,
            ..Default::default()
        },
    ] {
        let mut audio = SpeechAudio::new(config).expect("audio");
        audio.append(&pcm).expect("append");
        audio.score(0, &vec![1.0; samples / frame]).expect("score");
        let chunks = audio.take(true);
        if config.long_form {
            assert_eq!(chunks.len(), 1);
        } else {
            assert!(chunks.iter().all(|c| c.len() <= 32000));
        }
        assert_eq!(chunks.into_iter().flatten().collect::<Vec<_>>(), pcm);
    }
}

#[test]
fn speech_framing_pcm_validation_and_language_hints_match_typescript() {
    for bytes in [
        vec![],
        vec![0; 3],
        f32::NAN.to_le_bytes().to_vec(),
        f32::INFINITY.to_le_bytes().to_vec(),
        1.1f32.to_le_bytes().to_vec(),
        vec![0; MAX_SPEECH_BYTES + 4],
    ] {
        assert_eq!(decode_pcm(&bytes), Err(SpeechError::InvalidAudio));
    }
    let pooled = [9u8, 9, 0, 0, 128, 62, 9];
    assert_eq!(decode_pcm(&pooled[2..6]).expect("slice"), vec![0.25]);
    let options = serde_json::json!({"speechLanguage":"auto","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":true});
    let metadata = serde_json::to_vec(&options).expect("json");
    let mut body = (metadata.len() as u32).to_le_bytes().to_vec();
    body.extend(metadata);
    body.extend(0.25f32.to_le_bytes());
    let (options, pcm) = decode_request(&body).expect("request");
    assert!(options.speech_silence_filtering);
    assert_eq!(decode_pcm(pcm).expect("pcm"), vec![0.25]);
    let regional = vec![
        "en-US".into(),
        "en-GB".into(),
        "es-ES".into(),
        "de-DE".into(),
    ];
    for (requested, expected) in [
        ("auto", None),
        ("DE_de", Some("de-DE")),
        ("en", Some("en-US")),
        ("de-AT", Some("de-AT")),
        ("fr", Some("fr")),
    ] {
        assert_eq!(native_language(&regional, requested).as_deref(), expected);
    }
    assert!(!platform_supported("win32", "arm64", None));
    assert!(platform_supported("win32", "x64", None));
    assert!(!platform_supported("linux", "x64", Some("musl")));
}
