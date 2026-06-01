use std::io::Read;

use anyhow::{Context, Result, bail};

use crate::cli::Cli;

pub struct Command<'a> {
    pub line: &'a str,
    pub name: Option<&'a str>,
}

impl Command<'_> {
    pub fn label(&self) -> &str {
        self.name.unwrap_or(self.line)
    }
}

pub fn from_cli(cli: &Cli) -> Vec<Command<'_>> {
    cli.commands
        .iter()
        .enumerate()
        .map(|(idx, line)| Command {
            line,
            name: cli.command_name.get(idx).map(String::as_str),
        })
        .collect()
}

/// Read commands from stdin, one (non-blank) per line, for the `-` positional.
pub fn from_stdin() -> Result<Vec<String>> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("failed to read commands from stdin")?;
    let commands: Vec<String> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    if commands.is_empty() {
        bail!("no commands read from stdin");
    }
    Ok(commands)
}
