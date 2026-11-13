//! Small compatibility probe: all paths must be explicitly supplied by the caller.
use std::{env, path::PathBuf};

use t3_persistence::{Database, migrations, rusqlite::Connection};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let path = PathBuf::from(arguments.next().ok_or("expected database path")?);
    if let Some(head) = arguments.next() {
        let head = head.to_str().ok_or("invalid head")?.parse()?;
        let mut connection = Connection::open(path)?;
        let applied = migrations::run_migrations(&mut connection, Some(head))?;
        println!("Rust applied {} migrations through {head}", applied.len());
    } else {
        drop(Database::open(path)?);
        println!("Rust opened database at head 89");
    }
    Ok(())
}
