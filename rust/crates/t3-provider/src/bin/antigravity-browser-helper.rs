use std::io::Write;
fn main() {
    if let Some(url) = std::env::args().nth(1) {
        let message = format!(
            "__T3_ANTIGRAVITY_AUTH_URL__{}\n",
            serde_json::to_string(&url).unwrap_or_default()
        );
        let _ = std::io::stderr().write_all(message.as_bytes());
    }
}
