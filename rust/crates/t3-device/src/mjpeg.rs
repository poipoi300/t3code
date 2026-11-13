use crate::io::ByteStream;
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use std::time::Duration;

#[derive(Default)]
pub struct PartTracker {
    pending: BytesMut,
    end: Option<usize>,
    disabled: bool,
    still: Option<Bytes>,
}
impl PartTracker {
    pub fn feed(&mut self, chunk: &[u8]) {
        if self.disabled {
            return;
        }
        let mut remaining = chunk;
        while !remaining.is_empty() {
            if self.end.is_none() {
                let needed = (16usize * 1024 + 4)
                    .saturating_sub(self.pending.len())
                    .min(remaining.len());
                self.pending.extend_from_slice(&remaining[..needed]);
                remaining = &remaining[needed..];
                let header_end = self.pending.windows(4).position(|w| w == b"\r\n\r\n");
                if let Some(header_end) = header_end {
                    if header_end > 16 * 1024 {
                        self.disable();
                        return;
                    }
                    let Ok(header) = std::str::from_utf8(&self.pending[..header_end]) else {
                        self.disable();
                        return;
                    };
                    let length = header.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        (key.eq_ignore_ascii_case("content-length"))
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    });
                    let Some(length) = length
                        .filter(|n| *n <= 16 * 1024 * 1024)
                        .filter(|_| header.starts_with("--"))
                    else {
                        self.disable();
                        return;
                    };
                    self.end = Some(header_end + 4 + length + 2);
                } else if self.pending.len() >= 16 * 1024 + 4 {
                    self.disable();
                    return;
                } else {
                    self.still = None;
                    return;
                }
            }
            let Some(end) = self.end else { return };
            if self.pending.len() < end {
                let taken = (end - self.pending.len()).min(remaining.len());
                self.pending.extend_from_slice(&remaining[..taken]);
                remaining = &remaining[taken..];
            }
            if self.pending.len() < end {
                self.still = None;
                return;
            }
            let complete = self.pending.split_to(end).freeze();
            if !complete.ends_with(b"\r\n") {
                self.disable();
                return;
            }
            self.still = Some(complete);
            self.end = None;
            if !self.pending.is_empty() {
                let tail = self.pending.split().freeze();
                self.feed(&tail);
            }
        }
    }
    fn disable(&mut self) {
        self.disabled = true;
        self.pending.clear();
        self.still = None;
    }
    pub fn take_still(&mut self) -> Option<Bytes> {
        self.still.take()
    }
}
pub fn repeat_still_frame(mut upstream: ByteStream) -> ByteStream {
    Box::pin(async_stream::try_stream! {
        let mut tracker=PartTracker::default();
        loop{
            if let Some(still)=tracker.take_still(){
                let next=upstream.next();tokio::pin!(next);
                match tokio::time::timeout(Duration::from_millis(100),&mut next).await{
                    Ok(Some(chunk))=>{let chunk=chunk?;tracker.feed(&chunk);yield chunk;},
                    Ok(None)=>break,
                    Err(_)=>{yield still;if let Some(chunk)=next.await{let chunk=chunk?;tracker.feed(&chunk);yield chunk;}else{break;}}
                }
            }else{match upstream.next().await{Some(chunk)=>{let chunk=chunk?;tracker.feed(&chunk);yield chunk;},None=>break}}
        }
    })
}
