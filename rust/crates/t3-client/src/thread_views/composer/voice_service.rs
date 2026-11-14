//! Start/stop blob capture cannot implement the bounded PCM contract in voice::VoiceController.
use crate::{ClientError, voice::VoiceInputState};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
pub const VOICE_UNAVAILABLE_REASON: &str = "Voice transcription is not available.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerVoiceView {
    pub state: VoiceInputState,
    pub available: bool,
    pub unavailable_reason: Option<&'static str>,
}
#[derive(Clone)]
pub struct ComposerVoiceSession {
    view: Rc<ComposerVoiceView>,
}
impl Default for ComposerVoiceSession {
    fn default() -> Self {
        Self {
            view: Rc::new(ComposerVoiceView {
                state: VoiceInputState::default(),
                available: false,
                unavailable_reason: Some(VOICE_UNAVAILABLE_REASON),
            }),
        }
    }
}
impl ComposerVoiceSession {
    pub fn snapshot(&self) -> Rc<ComposerVoiceView> {
        self.view.clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::once(async {}).boxed_local()
    }
    pub fn start(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Err(ClientError::Unsupported(VOICE_UNAVAILABLE_REASON)) }.boxed_local()
    }
    pub fn stop(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.start()
    }
    pub fn cancel(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.start()
    }
}
