//! Folding work per write must follow the runs a write touches, not the line's length, as
//! TypeScript's does: the server folds under the command-output hub's lock. Work is counted as
//! bytes allocated (copying a line in Rust allocates it), never as elapsed time.
#![allow(clippy::unwrap_used)]
#![cfg(not(target_arch = "wasm32"))]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
use t3_shared::terminal_output::{TERMINAL_OUTPUT_TAIL_CHARS, TerminalOutput};

struct Counting;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
// SAFETY: every call forwards to the system allocator unchanged; the wrapper only counts bytes.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocated_by(work: impl FnOnce()) -> usize {
    let before = ALLOCATED.load(Ordering::Relaxed);
    work();
    ALLOCATED.load(Ordering::Relaxed) - before
}

// One test in this binary, so no other test allocates while it counts.
#[test]
fn rewrites_and_colour_runs_on_a_long_line_do_not_copy_the_line_per_write() {
    let mut output = TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS);
    output.push(&"L".repeat(TERMINAL_OUTPUT_TAIL_CHARS));
    let chunks: Vec<String> = (0..10).map(|_| "\rX".repeat(1_000)).collect();
    // 10,000 carriage-return rewrites; copying the 64K-unit line once per write is about 1.3 GB.
    let rewrites = allocated_by(|| {
        for chunk in &chunks {
            output.push(chunk);
        }
    });
    assert!(rewrites < 1 << 20, "{rewrites} bytes for 10,000 rewrites");
    let text = output.snapshot().text;
    assert!(text.starts_with("XL"));
    assert_eq!(text.encode_utf16().count(), TERMINAL_OUTPUT_TAIL_CHARS);

    // A colour per character splits the long run at the cursor on every write: a few small runs
    // per write (about 4.5 MB in all), where copying the line per write is about 1.3 GB.
    let colours: Vec<String> = (0..10)
        .map(|_| {
            (0..1_000)
                .map(|i| format!("\x1b[3{}mC", i % 7 + 1))
                .collect()
        })
        .collect();
    let split = allocated_by(|| {
        for chunk in &colours {
            output.push(chunk);
        }
    });
    assert!(
        split < 16 << 20,
        "{split} bytes for 10,000 coloured rewrites"
    );

    // At the end of a line over the cap, every write also trims the line's start.
    output.push("\x1b[99999G");
    let trimmed = allocated_by(|| {
        for chunk in &colours {
            output.push(chunk);
        }
    });
    assert!(
        trimmed < 16 << 20,
        "{trimmed} bytes for 10,000 writes past the cap"
    );
    let snapshot = output.snapshot();
    assert!(snapshot.truncated);
    assert!(snapshot.text.ends_with("C\x1b[0m"));

    // A pen that alternates in the middle of a long run splits it and merges it back on every
    // cycle. TypeScript's sliced and concatenated strings stay flat in the line's length here
    // (3.6 to 4.2 µs per cycle at 1K and at 64K one-byte units); copying half the line per write
    // is about 660 MB.
    let mut output = TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS);
    output.push(&"L".repeat(TERMINAL_OUTPUT_TAIL_CHARS));
    output.push("\x1b[32769G");
    let cycles: Vec<String> = (0..10)
        .map(|_| "\x1b[31mX\x08\x1b[0mL\x08".repeat(1_000))
        .collect();
    let alternating = allocated_by(|| {
        for chunk in &cycles {
            output.push(chunk);
        }
    });
    assert!(
        alternating < 4 << 20,
        "{alternating} bytes for 10,000 alternating-pen cycles"
    );
    assert_eq!(output.snapshot().text, "L".repeat(TERMINAL_OUTPUT_TAIL_CHARS));
}
