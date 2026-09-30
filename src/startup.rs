//! Private, inherited pipes bind startup to the child created by this CLI.
use crate::session::SessionFile;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "startup", rename_all = "snake_case")]
pub enum Message {
    Prepared { run_id: String, pid: u32 },
    Ready { session: SessionFile },
    Started { run_id: String },
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command { Start, Commit }

pub fn write_json(output: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

pub fn read_json<T: serde::de::DeserializeOwned>(input: &mut impl BufRead) -> Result<T> {
    let mut line = String::new();
    let count = input.take(65_537).read_line(&mut line)?;
    ensure!(count > 0, "startup caller disconnected; recording was not committed");
    ensure!(count <= 65_536 && line.ends_with('\n'), "invalid startup message");
    serde_json::from_str(&line).context("invalid startup message")
}

pub fn expect(input: &mut impl BufRead, command: Command) -> Result<()> {
    ensure!(read_json::<Command>(input)? == command, "unexpected startup command");
    Ok(())
}
