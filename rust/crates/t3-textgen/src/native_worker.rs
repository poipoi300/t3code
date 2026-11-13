use crate::{
    audio::{AudioConfig, SpeechAudio},
    native::{MAX_WORKER_FRAME, WorkerCommand, WorkerReply},
    speech::{
        EngineCapabilities, MAX_SPEECH_BYTES, NativeOptions, SpeechError, StreamUpdate, decode_pcm,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::{Read, Write};
use transcribe_cpp::{
    Backend, DeviceType, Feature, Model, ModelOptions, RunOptions, Session, Stream, Task,
    TimestampKind, VadSession,
};

fn read_command(reader: &mut impl Read) -> Result<Option<WorkerCommand>, SpeechError> {
    let mut length = [0; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(_) => return Err(SpeechError::Inference),
    }
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_WORKER_FRAME {
        return Err(SpeechError::InvalidAudio);
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| SpeechError::Inference)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| SpeechError::Inference)
}
fn reply(writer: &mut impl Write, reply: WorkerReply) -> Result<(), SpeechError> {
    let bytes = serde_json::to_vec(&reply).map_err(|_| SpeechError::Inference)?;
    if bytes.len() > MAX_WORKER_FRAME {
        return Err(SpeechError::Inference);
    }
    writer
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|_| SpeechError::Inference)?;
    writer
        .write_all(&bytes)
        .map_err(|_| SpeechError::Inference)?;
    writer.flush().map_err(|_| SpeechError::Inference)
}
fn decode(value: &str) -> Result<Vec<f32>, SpeechError> {
    if value.len() > MAX_SPEECH_BYTES.div_ceil(3) * 4 {
        return Err(SpeechError::InvalidAudio);
    }
    decode_pcm(
        &STANDARD
            .decode(value)
            .map_err(|_| SpeechError::InvalidAudio)?,
    )
}
fn capabilities(model: &Model) -> Result<EngineCapabilities, SpeechError> {
    let cap = model.capabilities().map_err(|_| SpeechError::Inference)?;
    Ok(EngineCapabilities {
        backend: model.backend(),
        languages: cap.languages,
        streaming: cap.supports_streaming,
        long_form: model.supports(Feature::LongForm),
        translation: cap.supports_translate,
        vocabulary: model.supports(Feature::Vocabulary),
        max_audio_ms: cap.max_audio_ms.max(0) as u64,
    })
}
fn run_options(options: &NativeOptions) -> RunOptions {
    RunOptions {
        language: options.language.clone(),
        task: if options.translate {
            Task::Translate
        } else {
            Task::Transcribe
        },
        target_language: options.translate.then(|| "en".into()),
        timestamps: TimestampKind::None,
        vocabulary: options.vocabulary.clone(),
        ..Default::default()
    }
}
fn offline(
    session: &mut Session,
    pcm: &[f32],
    options: &RunOptions,
    texts: &mut Vec<String>,
) -> Result<(), SpeechError> {
    match session.run(pcm, options) {
        Ok(result) => {
            texts.push(result.text);
            Ok(())
        }
        Err(
            transcribe_cpp::Error::InputTooLong(_) | transcribe_cpp::Error::OutputTruncated { .. },
        ) if pcm.len() > 4000 => {
            let mid = pcm.len() / 2;
            offline(session, &pcm[..mid], options, texts)?;
            offline(session, &pcm[mid..], options, texts)
        }
        Err(_) => Err(SpeechError::Inference),
    }
}
enum Decoder<'a> {
    Streaming(Stream<'a>),
    Offline(&'a mut Session, Box<RunOptions>, Vec<String>),
}
impl Decoder<'_> {
    fn feed(
        &mut self,
        pcm: &[f32],
    ) -> Result<Option<t3_contracts::speech::SpeechStreamText>, SpeechError> {
        match self {
            Self::Streaming(stream) => {
                let update = stream.feed(pcm).map_err(|_| SpeechError::Inference)?;
                if update.committed_changed || update.tentative_changed {
                    let text = stream.text();
                    Ok(Some(t3_contracts::speech::SpeechStreamText {
                        committed: text.committed,
                        tentative: text.tentative,
                    }))
                } else {
                    Ok(None)
                }
            }
            Self::Offline(session, options, texts) => {
                offline(session, pcm, options, texts)?;
                Ok(Some(t3_contracts::speech::SpeechStreamText {
                    committed: texts.join(" ").trim().into(),
                    tentative: String::new(),
                }))
            }
        }
    }
    fn finish(&mut self, heard: bool) -> Result<String, SpeechError> {
        match self {
            Self::Streaming(stream) => {
                if !heard {
                    return Ok(String::new());
                }
                stream.finalize().map_err(|_| SpeechError::Inference)?;
                Ok(stream.text().full)
            }
            Self::Offline(_, _, texts) => Ok(texts.join(" ").trim().into()),
        }
    }
}
enum Outcome {
    Finished,
    Reset,
    Restart { raw: Vec<f32>, final_audio: bool },
}
struct Recording {
    audio: SpeechAudio,
    vad: Option<VadSession>,
    decoder_heard: bool,
    revision: i64,
    bytes: usize,
}
fn score(
    recording: &mut Recording,
    pcm: Option<&[f32]>,
    final_audio: bool,
) -> Result<(), SpeechError> {
    if let Some(vad) = recording.vad.as_mut() {
        if let Some(pcm) = pcm {
            let result = vad.stream_feed(pcm).map_err(|_| SpeechError::InvalidVad)?;
            recording
                .audio
                .score(result.first_frame as usize, &result.probs)?;
            if result.first_frame as usize + result.probs.len() != recording.audio.samples() / 512 {
                return Err(SpeechError::InvalidVad);
            }
        }
        if final_audio {
            let result = vad.stream_flush().map_err(|_| SpeechError::InvalidVad)?;
            recording
                .audio
                .score(result.first_frame as usize, &result.probs)?;
            if result.first_frame as usize + result.probs.len()
                != recording.audio.samples().div_ceil(512)
            {
                return Err(SpeechError::InvalidVad);
            }
        }
    } else {
        let count = if final_audio {
            recording.audio.samples().div_ceil(512)
        } else {
            recording.audio.samples() / 512
        };
        let scored = recording.audio.scored_frames();
        recording.audio.score(scored, &vec![1.0; count - scored])?;
    }
    Ok(())
}
fn drive(
    reader: &mut impl Read,
    writer: &mut impl Write,
    mut decoder: Decoder<'_>,
    mut recording: Recording,
    initial: Option<(Vec<f32>, bool)>,
    batch: bool,
) -> Result<(Outcome, i64), SpeechError> {
    let mut pending = initial;
    loop {
        let (pcm, final_audio) = if let Some((pcm, final_audio)) = pending.take() {
            (Some(pcm), final_audio)
        } else {
            match read_command(reader)? {
                Some(WorkerCommand::Feed { pcm }) => (Some(decode(&pcm)?), false),
                Some(WorkerCommand::Finish) => (None, true),
                Some(WorkerCommand::Reset) => {
                    reply(writer, WorkerReply::empty())?;
                    return Ok((Outcome::Reset, recording.revision));
                }
                None => return Ok((Outcome::Finished, recording.revision)),
                _ => return Err(SpeechError::StreamNotReady),
            }
        };
        if let Some(pcm) = &pcm {
            if recording.bytes.saturating_add(pcm.len() * 4) > MAX_SPEECH_BYTES {
                return Err(SpeechError::InvalidAudio);
            }
            recording.bytes += pcm.len() * 4;
            recording.audio.append(pcm)?;
        }
        if score(&mut recording, pcm.as_deref(), final_audio).is_err() {
            return Ok((
                Outcome::Restart {
                    raw: recording.audio.audio().to_vec(),
                    final_audio,
                },
                recording.revision,
            ));
        }
        let mut preview = None;
        for pcm in recording.audio.take(final_audio) {
            if !pcm.is_empty() {
                recording.decoder_heard = true;
                if let Some(text) = decoder.feed(&pcm)? {
                    preview = Some(text);
                }
            }
        }
        let mut response = WorkerReply::empty();
        if final_audio {
            response.text = Some(decoder.finish(recording.decoder_heard)?);
            reply(writer, response)?;
            return Ok((Outcome::Finished, recording.revision));
        }
        if !batch {
            recording.revision += 1;
            response.update = Some(StreamUpdate {
                revision: recording.revision,
                text: preview,
            });
            reply(writer, response)?;
        }
    }
}
fn recording(
    reader: &mut impl Read,
    writer: &mut impl Write,
    model: &Model,
    options: NativeOptions,
    initial: Option<Vec<f32>>,
) -> Result<(), SpeechError> {
    let cap = capabilities(model)?;
    let native_streaming = initial.is_none() && cap.streaming;
    let batch = initial.is_some();
    let mut raw = initial.map(|pcm| (pcm, true));
    let mut revision = 0;
    let mut use_vad =
        options.vad_path.is_some() && (options.filter_silence || !cap.streaming && !cap.long_form);
    let mut announced = batch;
    loop {
        let vad = if use_vad {
            options
                .vad_path
                .as_ref()
                .and_then(|path| {
                    Model::load_with(
                        path,
                        &ModelOptions {
                            backend: Backend::Cpu,
                            ..Default::default()
                        },
                    )
                    .ok()
                })
                .and_then(|m| {
                    let info = m.vad_info().ok()?;
                    if info.sample_rate != 16000 || info.frame_samples != 512 {
                        return None;
                    }
                    m.vad_session_with(&transcribe_cpp::VadSessionOptions { n_threads: 1 })
                        .ok()
                })
        } else {
            None
        };
        let audio = SpeechAudio::new(AudioConfig {
            frame_samples: 512,
            filter_silence: vad.is_some() && options.filter_silence,
            native_streaming: cap.streaming,
            long_form: cap.long_form,
            max_audio_ms: cap.max_audio_ms,
        })?;
        let recording = Recording {
            audio,
            vad,
            decoder_heard: false,
            revision,
            bytes: 0,
        };
        let mut session = model.session().map_err(|_| SpeechError::Inference)?;
        let run = run_options(&options);
        let decoder = if native_streaming {
            Decoder::Streaming(
                session
                    .stream(&run, &Default::default())
                    .map_err(|_| SpeechError::Inference)?,
            )
        } else {
            Decoder::Offline(&mut session, Box::new(run), Vec::new())
        };
        if !announced {
            reply(writer, WorkerReply::empty())?;
            announced = true;
        }
        let (outcome, next_revision) =
            drive(reader, writer, decoder, recording, raw.take(), batch)?;
        revision = next_revision;
        match outcome {
            Outcome::Finished | Outcome::Reset => return Ok(()),
            Outcome::Restart {
                raw: pcm,
                final_audio,
            } => {
                use_vad = false;
                raw = Some((pcm, final_audio));
            }
        }
    }
}
pub fn run_worker() -> Result<(), SpeechError> {
    transcribe_cpp::disable_logging();
    let mut reader = std::io::stdin().lock();
    let mut writer = std::io::stdout().lock();
    let mut model = None;
    while let Some(command) = read_command(&mut reader)? {
        let operation = (|| {
            let mut response = WorkerReply::empty();
            match command {
                WorkerCommand::Discover => {
                    response.devices = Some(
                        transcribe_cpp::devices()
                            .into_iter()
                            .filter(|d| matches!(d.device_type, DeviceType::Gpu | DeviceType::Igpu))
                            .map(|d| t3_contracts::speech::SpeechGpuDevice {
                                id: serde_json::to_string(&(
                                    d.kind,
                                    d.device_id.unwrap_or_else(|| d.name.clone()),
                                ))
                                .expect("device identity"),
                                name: if d.description.is_empty() {
                                    d.name
                                } else {
                                    d.description
                                },
                            })
                            .collect(),
                    );
                }
                WorkerCommand::Load { path, acceleration } => {
                    let mut options = ModelOptions::default();
                    if acceleration == "cpu" {
                        options.backend = Backend::Cpu;
                    }
                    if let Some(id) = acceleration.strip_prefix("gpu:") {
                        let device = transcribe_cpp::devices()
                            .into_iter()
                            .filter(|d| matches!(d.device_type, DeviceType::Gpu | DeviceType::Igpu))
                            .find(|d| {
                                serde_json::to_string(&(
                                    &d.kind,
                                    d.device_id.as_ref().unwrap_or(&d.name),
                                ))
                                .ok()
                                .as_deref()
                                    == Some(id)
                            })
                            .ok_or(SpeechError::Inference)?;
                        options.device = Some(device);
                    } else if !matches!(acceleration.as_str(), "cpu" | "auto") {
                        return Err(SpeechError::InvalidOptions);
                    }
                    let loaded =
                        Model::load_with(path, &options).map_err(|_| SpeechError::Inference)?;
                    response.capabilities = Some(capabilities(&loaded)?);
                    model = Some(loaded);
                }
                WorkerCommand::Begin { options } => {
                    recording(
                        &mut reader,
                        &mut writer,
                        model.as_ref().ok_or(SpeechError::ModelNotFound)?,
                        options,
                        None,
                    )?;
                    return Ok(());
                }
                WorkerCommand::Transcribe { pcm, options } => {
                    recording(
                        &mut reader,
                        &mut writer,
                        model.as_ref().ok_or(SpeechError::ModelNotFound)?,
                        options,
                        Some(decode(&pcm)?),
                    )?;
                    return Ok(());
                }
                WorkerCommand::Reset => (),
                _ => return Err(SpeechError::StreamNotReady),
            }
            reply(&mut writer, response)
        })();
        if let Err(error) = operation {
            let mut response = WorkerReply::empty();
            response.error = Some(error);
            reply(&mut writer, response)?;
        }
    }
    Ok(())
}
