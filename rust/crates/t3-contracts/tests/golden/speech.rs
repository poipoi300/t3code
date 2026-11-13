use super::super::{Registry, register};
use t3_contracts::speech::*;
pub fn register_module(registry: &mut Registry) {
    register::<CloudSpeechApiKey>(registry, "CloudSpeechApiKey");
    register::<CloudSpeechError>(registry, "CloudSpeechError");
    register::<CloudSpeechKeys>(registry, "CloudSpeechKeys");
    register::<CloudSpeechModel>(registry, "CloudSpeechModel");
    register::<CloudSpeechModelId>(registry, "CloudSpeechModelId");
    register::<CloudSpeechModels>(registry, "CloudSpeechModels");
    register::<CloudSpeechProvider>(registry, "CloudSpeechProvider");
    register::<EnvironmentSpeechAccelerationRequest>(
        registry,
        "EnvironmentSpeechAccelerationRequest",
    );
    register::<EnvironmentSpeechModel>(registry, "EnvironmentSpeechModel");
    register::<EnvironmentSpeechModelLinkRequest>(registry, "EnvironmentSpeechModelLinkRequest");
    register::<EnvironmentSpeechModelRequest>(registry, "EnvironmentSpeechModelRequest");
    register::<EnvironmentSpeechModelState>(registry, "EnvironmentSpeechModelState");
    register::<EnvironmentSpeechModelUnloadTimeoutRequest>(
        registry,
        "EnvironmentSpeechModelUnloadTimeoutRequest",
    );
    register::<EnvironmentSpeechModels>(registry, "EnvironmentSpeechModels");
    register::<EnvironmentSpeechPostProcessingRequest>(
        registry,
        "EnvironmentSpeechPostProcessingRequest",
    );
    register::<EnvironmentSpeechPostProcessingResult>(
        registry,
        "EnvironmentSpeechPostProcessingResult",
    );
    register::<EnvironmentSpeechState>(registry, "EnvironmentSpeechState");
    register::<EnvironmentSpeechStatus>(registry, "EnvironmentSpeechStatus");
    register::<EnvironmentSpeechTranscriptionResult>(
        registry,
        "EnvironmentSpeechTranscriptionResult",
    );
    register::<SpeechAcceleration>(registry, "SpeechAcceleration");
    register::<SpeechBenchmarkTier>(registry, "SpeechBenchmarkTier");
    register::<SpeechCorrectionCue>(registry, "SpeechCorrectionCue");
    register::<SpeechCustomWords>(registry, "SpeechCustomWords");
    register::<SpeechGpuDevice>(registry, "SpeechGpuDevice");
    register::<SpeechLanguage>(registry, "SpeechLanguage");
    register::<SpeechModelId>(registry, "SpeechModelId");
    register::<SpeechModelUnloadTimeout>(registry, "SpeechModelUnloadTimeout");
    register::<SpeechPostProcessingOptions>(registry, "SpeechPostProcessingOptions");
    register::<SpeechPostProcessingPromptSettings>(registry, "SpeechPostProcessingPromptSettings");
    register::<SpeechStreamCommand>(registry, "SpeechStreamCommand");
    register::<SpeechStreamEvent>(registry, "SpeechStreamEvent");
    register::<SpeechStreamText>(registry, "SpeechStreamText");
    register::<SpeechTranscriptionOptions>(registry, "SpeechTranscriptionOptions");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("speech", register_module)
}

#[test]
fn legacy_benchmarks_encode_the_scalar_wire_format() -> Result<(), serde_json::Error> {
    use serde_json::json;
    for (cpu, accelerated) in [
        (None, Some(2)),
        (Some(None), None),
        (Some(Some(4)), Some(2)),
    ] {
        let mut times = json!({"accelerated":accelerated});
        let mut expected = json!({"accuracy":{},"secondsPer30s":accelerated});
        if let Some(cpu) = cpu {
            times["cpu"] = json!(cpu);
            expected["cpuSecondsPer30s"] = json!(cpu);
        }
        let decoded: SpeechBenchmark =
            serde_json::from_value(json!({"accuracy":{},"secondsPer30s":times}))?;
        assert_eq!(serde_json::to_value(decoded)?, expected);
    }
    Ok(())
}

#[test]
fn legacy_speech_status_infers_the_benchmark_tier() -> Result<(), serde_json::Error> {
    use serde_json::json;
    for (acceleration, devices, tier) in [
        ("cpu", json!([{"id":"gpu","name":"GPU"}]), "cpu"),
        ("auto", json!([]), "cpu"),
        ("auto", json!([{"id":"gpu","name":"GPU"}]), "accelerated"),
        ("gpu:0", json!([]), "accelerated"),
    ] {
        let mut value = json!({"supported":true,"state":"ready","modelId":"model","model":"Model","size":1,
   "supportsStreaming":true,"supportsTranslation":false,"acceleration":acceleration,
   "modelUnloadTimeout":"never","gpuDevices":devices});
        let decoded: EnvironmentSpeechStatus = serde_json::from_value(value.clone())?;
        value["benchmarkTier"] = json!(tier);
        assert_eq!(serde_json::to_value(decoded)?, value);
    }
    Ok(())
}
