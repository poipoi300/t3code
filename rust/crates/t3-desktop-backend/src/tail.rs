//! The last output of the server, kept for the "could not start" window, and the migration check
//! that stops a slow database upgrade from being reported as a failure.

use t3_client::view_logic::shell::startup_recovery::STARTUP_FAILURE_OUTPUT_CHARS;

/// `DATABASE_MIGRATIONS_STARTED_MESSAGE`.
const MIGRATIONS_STARTED: &str = "Running database migrations";
/// `DATABASE_MIGRATION_PROGRESS_MESSAGE`.
const MIGRATION_PROGRESS: &str = "Upgrading database";
/// `DATABASE_MIGRATIONS_FINISHED_MESSAGE`.
const MIGRATIONS_FINISHED: &str = "Migrations ran successfully";

/// `isRunningDatabaseMigrations`: the server logged that it started or progressed a migration after
/// the last time it logged that migrations finished.
pub fn is_running_database_migrations(output: &str) -> bool {
    let started = [MIGRATIONS_STARTED, MIGRATION_PROGRESS]
        .iter()
        .filter_map(|marker| output.rfind(marker))
        .max();
    started > output.rfind(MIGRATIONS_FINISHED)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Tail of both streams of one run. Each stream has its own UTF-8 decoder so a character split
/// across two reads stays whole; the text is cut on character boundaries.
#[derive(Debug, Default)]
pub struct OutputTail {
    text: String,
    pending: [Vec<u8>; 2],
}

impl OutputTail {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn push(&mut self, stream: Stream, bytes: &[u8]) {
        let pending = &mut self.pending[stream as usize];
        pending.extend_from_slice(bytes);
        let mut rest: &[u8] = pending;
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    self.text.push_str(valid);
                    rest = &[];
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    self.text
                        .push_str(std::str::from_utf8(valid).unwrap_or_default());
                    match error.error_len() {
                        // A character that continues in the next read.
                        None => {
                            rest = after;
                            break;
                        }
                        Some(length) => {
                            self.text.push('\u{FFFD}');
                            rest = &after[length..];
                        }
                    }
                }
            }
        }
        let kept = rest.to_vec();
        *pending = kept;
        let excess = self
            .text
            .chars()
            .count()
            .saturating_sub(STARTUP_FAILURE_OUTPUT_CHARS);
        if excess > 0 {
            let cut = self
                .text
                .char_indices()
                .nth(excess)
                .map_or(self.text.len(), |(index, _)| index);
            self.text.drain(..cut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_migration_in_progress_is_not_a_failure_until_it_finishes() {
        assert!(!is_running_database_migrations(""));
        assert!(!is_running_database_migrations("listening"));
        assert!(is_running_database_migrations(
            "Running database migrations"
        ));
        assert!(is_running_database_migrations(
            "Migrations ran successfully\nRunning database migrations\nUpgrading database 3"
        ));
        assert!(!is_running_database_migrations(
            "Running database migrations\nUpgrading database 3\nMigrations ran successfully"
        ));
    }

    #[test]
    fn a_character_split_between_reads_is_decoded_once_per_stream() {
        let mut tail = OutputTail::default();
        let bytes = "é".as_bytes();
        tail.push(Stream::Stdout, &bytes[..1]);
        tail.push(Stream::Stderr, b"x");
        tail.push(Stream::Stdout, &bytes[1..]);
        assert_eq!(tail.text(), "xé");
    }

    #[test]
    fn invalid_bytes_become_replacement_characters() {
        let mut tail = OutputTail::default();
        tail.push(Stream::Stdout, b"a\xFFb");
        assert_eq!(tail.text(), "a\u{FFFD}b");
    }

    #[test]
    fn the_tail_keeps_the_last_characters_on_a_boundary() {
        let mut tail = OutputTail::default();
        tail.push(
            Stream::Stdout,
            "é".repeat(STARTUP_FAILURE_OUTPUT_CHARS).as_bytes(),
        );
        tail.push(Stream::Stdout, b"end");
        assert_eq!(tail.text().chars().count(), STARTUP_FAILURE_OUTPUT_CHARS);
        assert!(tail.text().ends_with("éend"));
    }
}
