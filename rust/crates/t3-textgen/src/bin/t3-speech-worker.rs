fn main() {
    let status = if t3_textgen::native_worker::run_worker().is_ok() {
        0
    } else {
        1
    };
    // Native teardown during inference may fault; the worker never shares the host's address space.
    std::process::exit(status);
}
