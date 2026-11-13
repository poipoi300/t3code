use crate::{
    CancellationToken,
    dictionary::apply_aliases,
    generation::{
        Generated, GenerationContext, GenerationError, GenerationInput, RoutingState,
        TextGeneration,
    },
    prompts::{default_voice_instructions, voice_prompt},
};
use t3_contracts::speech::{
    EnvironmentSpeechPostProcessingRequest, EnvironmentSpeechPostProcessingResult, SpeechNumber,
    SpeechPostProcessingPromptMode,
};

fn offset(value: &SpeechNumber, len: usize) -> usize {
    let number = match value {
        SpeechNumber::Finite(n) => n.value(),
        SpeechNumber::NonFinite(t3_contracts::speech::NonFiniteSpeechNumber::Infinity) => {
            return len;
        }
        SpeechNumber::NonFinite(_) => return 0,
    };
    if number.is_nan() || number < 0.0 {
        0
    } else {
        (number as usize).min(len)
    }
}
pub async fn post_process(
    service: &TextGeneration,
    mut context: GenerationContext,
    input: EnvironmentSpeechPostProcessingRequest,
    routing: Option<&RoutingState>,
    cancellation: CancellationToken,
) -> Result<EnvironmentSpeechPostProcessingResult, GenerationError> {
    if input.transcript.trim().is_empty() {
        return Ok(EnvironmentSpeechPostProcessingResult {
            text: input.transcript,
        });
    }
    if crate::text::utf16_len(&input.transcript) > 100000
        || crate::text::utf16_len(&input.draft.text) > 100000
        || crate::text::utf16_len(
            &input
                .options
                .speech_post_processing_prompt
                .custom_instructions,
        ) > 10000
    {
        return Err(GenerationError::InvalidOutput);
    }
    let settings = &input.options.speech_post_processing_prompt;
    let custom = if matches!(settings.mode, SpeechPostProcessingPromptMode::Custom) {
        settings.custom_instructions.trim()
    } else {
        ""
    };
    let instructions = if custom.is_empty() {
        default_voice_instructions()
    } else {
        custom
    };
    let directory = tempfile::Builder::new()
        .prefix("t3-voice-cleanup-")
        .tempdir()
        .map_err(|_| GenerationError::Provider)?;
    context.cwd = directory.path().to_owned();
    let len = crate::text::utf16_len(&input.draft.text);
    let start = offset(&input.draft.selection.start, len);
    let end = offset(&input.draft.selection.end, len);
    let prompt = voice_prompt(
        instructions,
        &input.transcript,
        &input.draft.text,
        start,
        end,
        input
            .options
            .speech_correction_word
            .as_deref()
            .unwrap_or(""),
    );
    match service
        .generate(
            context,
            GenerationInput::Transcription { prompt },
            routing,
            cancellation,
        )
        .await?
    {
        Generated::Transcription { transcription } => Ok(EnvironmentSpeechPostProcessingResult {
            text: if transcription.trim().is_empty() {
                input.transcript
            } else {
                apply_aliases(transcription.trim(), &input.options.speech_custom_words)
            },
        }),
        _ => Err(GenerationError::InvalidOutput),
    }
}
