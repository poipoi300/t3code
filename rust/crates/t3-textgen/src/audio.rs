use crate::speech::{MAX_SPEECH_BYTES, SAMPLE_RATE, SpeechError};

#[derive(Clone, Copy)]
pub struct AudioConfig {
    pub frame_samples: usize,
    pub filter_silence: bool,
    pub native_streaming: bool,
    pub long_form: bool,
    pub max_audio_ms: u64,
}
impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            frame_samples: 512,
            filter_silence: true,
            native_streaming: false,
            long_form: false,
            max_audio_ms: 0,
        }
    }
}

pub struct SpeechAudio {
    config: AudioConfig,
    pcm: Vec<f32>,
    probs: Vec<f32>,
    keep: Vec<bool>,
    start: usize,
    stream_at: usize,
    in_speech: bool,
    onset_count: usize,
    hangover: usize,
}
impl SpeechAudio {
    pub fn new(config: AudioConfig) -> Result<Self, SpeechError> {
        if config.frame_samples == 0 {
            return Err(SpeechError::InvalidAudio);
        }
        Ok(Self {
            config,
            pcm: Vec::new(),
            probs: Vec::new(),
            keep: Vec::new(),
            start: 0,
            stream_at: 0,
            in_speech: false,
            onset_count: 0,
            hangover: 0,
        })
    }
    pub fn append(&mut self, pcm: &[f32]) -> Result<(), SpeechError> {
        if self.pcm.len().saturating_add(pcm.len()) > MAX_SPEECH_BYTES / 4 {
            return Err(SpeechError::InvalidAudio);
        }
        self.pcm.extend_from_slice(pcm);
        Ok(())
    }
    pub fn audio(&self) -> &[f32] {
        &self.pcm
    }
    pub fn samples(&self) -> usize {
        self.pcm.len()
    }
    pub fn scored_frames(&self) -> usize {
        self.probs.len()
    }
    fn frames(&self, ms: usize) -> usize {
        (ms * SAMPLE_RATE / 1000).div_ceil(self.config.frame_samples)
    }
    pub fn score(&mut self, first: usize, scores: &[f32]) -> Result<(), SpeechError> {
        if first != self.probs.len()
            || scores
                .iter()
                .any(|p| !p.is_finite() || *p < 0.0 || *p > 1.0)
            || first.saturating_add(scores.len())
                > self.pcm.len().div_ceil(self.config.frame_samples)
        {
            return Err(SpeechError::InvalidVad);
        }
        let prefill = self.frames(450);
        let onset = self.frames(60);
        let tail = self.frames(if self.config.native_streaming {
            1650
        } else {
            450
        });
        for &probability in scores {
            let i = self.probs.len();
            self.probs.push(probability);
            self.keep.push(false);
            let voiced = probability > 0.3;
            if !self.in_speech && voiced {
                self.onset_count += 1;
                if self.onset_count >= onset {
                    self.in_speech = true;
                    self.hangover = tail;
                    self.onset_count = 0;
                    for j in i.saturating_sub(prefill)..=i {
                        self.keep[j] = true;
                    }
                }
            } else if self.in_speech && voiced {
                self.hangover = tail;
                self.keep[i] = true;
            } else if self.in_speech && self.hangover > 0 {
                self.hangover -= 1;
                self.keep[i] = true;
            } else {
                self.in_speech = false;
                self.onset_count = 0;
            }
        }
        Ok(())
    }
    fn read(&self, from: usize, to: usize) -> Vec<f32> {
        if !self.config.filter_silence {
            return self.pcm[from..to].to_vec();
        }
        let mut output = Vec::new();
        let mut at = from;
        let frame = self.config.frame_samples;
        while at < to {
            let end = to.min((at / frame + 1) * frame);
            if self.keep.get(at / frame) == Some(&true) {
                output.extend_from_slice(&self.pcm[at..end]);
            }
            at = end;
        }
        output
    }
    fn pauses(&self, samples: usize, final_audio: bool) -> Vec<(usize, f64)> {
        let mut segments = Vec::new();
        let mut speech = None;
        let mut silence = None;
        for (i, p) in self.probs.iter().enumerate() {
            let at = i * self.config.frame_samples;
            if *p >= 0.5 {
                silence = None;
                speech.get_or_insert(at);
            } else if *p < 0.35 && speech.is_some() {
                let silence_at = *silence.get_or_insert(at);
                if at - silence_at >= SAMPLE_RATE / 10 {
                    let speech_at = speech.unwrap_or(0);
                    if silence_at - speech_at > SAMPLE_RATE / 4 {
                        segments.push((speech_at, silence_at));
                    }
                    speech = None;
                    silence = None;
                }
            }
        }
        if let Some(at) = speech
            && samples.saturating_sub(at) > SAMPLE_RATE / 4
        {
            segments.push((at, samples));
        }
        let mut candidates = Vec::new();
        let mut end = 0;
        for (start, finish) in segments {
            if start > end {
                candidates.push(((end + start) / 2, (start - end) as f64 / SAMPLE_RATE as f64));
            }
            end = finish;
        }
        if final_audio && samples > end {
            candidates.push((
                (end + samples) / 2,
                (samples - end) as f64 / SAMPLE_RATE as f64,
            ));
        }
        candidates
    }
    pub fn take(&mut self, final_audio: bool) -> Vec<Vec<f32>> {
        let frame = self.config.frame_samples;
        let samples = self.pcm.len().min(self.probs.len() * frame);
        let prefill = self.frames(450);
        if final_audio && self.onset_count > 0 {
            for i in self.probs.len().saturating_sub(1 + prefill)..self.probs.len() {
                self.keep[i] = true;
            }
        }
        if self.config.native_streaming {
            let output = self.read(self.stream_at, samples);
            let mut last = self.stream_at;
            for i in self.stream_at / frame..self.probs.len() {
                if !self.config.filter_silence || self.keep[i] {
                    last = samples.min((i + 1) * frame);
                }
            }
            self.stream_at = last.max(samples.saturating_sub(prefill * frame));
            return if output.is_empty() {
                Vec::new()
            } else {
                vec![output]
            };
        }
        let maximum = (40 * SAMPLE_RATE).min(if self.config.max_audio_ms > 0 {
            ((self.config.max_audio_ms as usize * SAMPLE_RATE) / 2000).max(1)
        } else {
            40 * SAMPLE_RATE
        });
        let minimum = (5 * SAMPLE_RATE).min(maximum / 4);
        let target = (30 * SAMPLE_RATE) as f64;
        let target = target.min(maximum as f64 * 0.75);
        let lookahead = minimum.max((prefill + self.frames(60)) * frame);
        let threshold = maximum + if final_audio { 0 } else { lookahead };
        let mut chunks = Vec::new();
        if !self.config.long_form && samples - self.start > threshold {
            let candidates = self.pauses(samples, final_audio);
            while samples - self.start > threshold {
                let mut best = None::<(usize, f64)>;
                let mut fallback = None::<(usize, f64)>;
                for &(cut, gap) in &candidates {
                    let Some(position) = cut.checked_sub(self.start) else {
                        continue;
                    };
                    if position < minimum || position > maximum {
                        continue;
                    }
                    let off_target = (position as f64 - target) / SAMPLE_RATE as f64;
                    let score =
                        gap.min(1.0) - off_target.abs() * if off_target > 0.0 { 0.1 } else { 0.05 };
                    if fallback.is_none_or(|(_, s)| score > s) {
                        fallback = Some((cut, score));
                    }
                    if samples - cut >= minimum && best.is_none_or(|(_, s)| score > s) {
                        best = Some((cut, score));
                    }
                }
                let hard = self.start
                    + if samples - self.start < maximum + minimum {
                        (samples - self.start) / 2
                    } else {
                        maximum
                    };
                let cut = best.or(fallback).map_or(hard, |(cut, _)| cut);
                let chunk = self.read(self.start, cut);
                if !chunk.is_empty() {
                    chunks.push(chunk);
                }
                self.start = cut;
            }
        }
        if final_audio && samples > self.start {
            let chunk = self.read(self.start, samples);
            if !chunk.is_empty() {
                chunks.push(chunk);
            }
            self.start = samples;
        }
        chunks
    }
}
