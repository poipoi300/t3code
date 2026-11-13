use crate::{
    api::{IdGenerator, IdKind},
    testing::ManualClock,
};
use std::sync::Mutex;

pub const DEFAULT_RANDOM_SEED: u32 = 0x1234_5678;
pub const FIXTURE_TIME: &str = "1970-01-01T00:00:00.000Z";

pub fn deterministic_runtime() -> (ManualClock, SeededIds) {
    (
        ManualClock::new(FIXTURE_TIME),
        SeededIds::new(DEFAULT_RANDOM_SEED),
    )
}

// ISAAC from Effect Random.ts; retained redistribution notices are in this package's README.
struct Isaac {
    memory: [u32; 256],
    results: [u32; 256],
    accumulator: u32,
    last_result: u32,
    counter: u32,
    remaining: usize,
}
impl Isaac {
    fn new(seed: u32) -> Self {
        let mut rng = Self {
            memory: [0; 256],
            results: [0; 256],
            accumulator: 0,
            last_result: 0,
            counter: 0,
            remaining: 0,
        };
        rng.results[0] = seed;
        let mut state = [0x9e37_79b9_u32; 8];
        for _ in 0..4 {
            Self::mix(&mut state);
        }
        for pass in 0..2 {
            for index in (0..256).step_by(8) {
                for (offset, value) in state.iter_mut().enumerate() {
                    *value = value.wrapping_add(if pass == 0 {
                        rng.results[index + offset]
                    } else {
                        rng.memory[index + offset]
                    });
                }
                Self::mix(&mut state);
                rng.memory[index..index + 8].copy_from_slice(&state);
            }
        }
        rng.generate();
        rng
    }
    fn mix(v: &mut [u32; 8]) {
        v[0] ^= v[1] << 11;
        v[3] = v[3].wrapping_add(v[0]);
        v[1] = v[1].wrapping_add(v[2]);
        v[1] ^= v[2] >> 2;
        v[4] = v[4].wrapping_add(v[1]);
        v[2] = v[2].wrapping_add(v[3]);
        v[2] ^= v[3] << 8;
        v[5] = v[5].wrapping_add(v[2]);
        v[3] = v[3].wrapping_add(v[4]);
        v[3] ^= v[4] >> 16;
        v[6] = v[6].wrapping_add(v[3]);
        v[4] = v[4].wrapping_add(v[5]);
        v[4] ^= v[5] << 10;
        v[7] = v[7].wrapping_add(v[4]);
        v[5] = v[5].wrapping_add(v[6]);
        v[5] ^= v[6] >> 4;
        v[0] = v[0].wrapping_add(v[5]);
        v[6] = v[6].wrapping_add(v[7]);
        v[6] ^= v[7] << 8;
        v[1] = v[1].wrapping_add(v[6]);
        v[7] = v[7].wrapping_add(v[0]);
        v[7] ^= v[0] >> 9;
        v[2] = v[2].wrapping_add(v[7]);
        v[0] = v[0].wrapping_add(v[1]);
    }
    fn generate(&mut self) {
        self.counter = self.counter.wrapping_add(1);
        self.last_result = self.last_result.wrapping_add(self.counter);
        for index in 0..256 {
            self.accumulator ^= match index & 3 {
                0 => self.accumulator << 13,
                1 => self.accumulator >> 6,
                2 => self.accumulator << 2,
                _ => self.accumulator >> 16,
            };
            self.accumulator = self
                .accumulator
                .wrapping_add(self.memory[(index + 128) & 255]);
            let x = self.memory[index];
            let y = self.memory[((x >> 2) & 255) as usize]
                .wrapping_add(self.accumulator.wrapping_add(self.last_result));
            self.memory[index] = y;
            self.last_result = self.memory[((y >> 10) & 255) as usize].wrapping_add(x);
            self.results[index] = self.last_result;
        }
        self.remaining = 256;
    }
    fn next_u32(&mut self) -> u32 {
        if self.remaining == 0 {
            self.generate();
        }
        self.remaining -= 1;
        self.results[self.remaining]
    }
    fn uuid_v4(&mut self) -> String {
        let mut bytes = [0_u8; 16];
        for byte in &mut bytes {
            let hi = self.next_u32() & 0x1f_ffff;
            let lo = self.next_u32();
            *byte = ((u64::from(hi) << 32 | u64::from(lo)) >> 45) as u8;
        }
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        let mut value = String::with_capacity(36);
        for (index, byte) in bytes.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                value.push('-');
            }
            value.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            value.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
        }
        value
    }
}

pub struct SeededIds(Mutex<Isaac>);
impl SeededIds {
    pub fn new(seed: u32) -> Self {
        Self(Mutex::new(Isaac::new(seed)))
    }
    pub fn uuid_v4(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .uuid_v4()
    }
    pub fn allocate(&self, prefix: &str, parts: &[&str]) -> String {
        let mut parts = parts.to_vec();
        let uuid = self.uuid_v4();
        parts.push(&uuid);
        crate::ports::decisions::orchestrator::derived_id(prefix, &parts)
    }
}
impl IdGenerator for SeededIds {
    fn next(&self, kind: IdKind) -> String {
        let prefix = match kind {
            IdKind::Event => "event",
            IdKind::Command => "command",
            IdKind::Run => "run",
            IdKind::RunAttempt => "run-attempt",
            IdKind::Node => "node",
            IdKind::Message => "message",
            IdKind::ProviderSession => "provider-session",
            IdKind::ProviderThread => "provider-thread",
            IdKind::ProviderTurn => "provider-turn",
            IdKind::TurnItem => "turn-item",
            IdKind::Checkpoint => "checkpoint",
            IdKind::CheckpointScope => "checkpoint-scope",
            IdKind::ContextTransfer => "context-transfer",
            IdKind::ContextHandoff => "context-handoff",
            IdKind::RuntimeRequest => "runtime-request",
            IdKind::Thread => "thread",
        };
        self.allocate(prefix, &[])
    }
}
