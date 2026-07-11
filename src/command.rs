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
    let names = cli.command_name.as_deref().unwrap_or_default();
    cli.commands
        .iter()
        .enumerate()
        .map(|(idx, line)| Command {
            line,
            name: names.get(idx).map(String::as_str),
        })
        .collect()
}

/// Automatic names for a bare `-n`: strip the prefix and suffix common to all
/// commands, keeping their distinctive middle. A command left empty (e.g. the
/// commands are identical) falls back to its full command line.
pub fn auto_names(commands: &[String]) -> Vec<String> {
    let chars: Vec<Vec<char>> = commands.iter().map(|c| c.chars().collect()).collect();
    let prefix = chars[1..].iter().fold(chars[0].len(), |len, c| {
        chars[0][..len]
            .iter()
            .zip(c)
            .take_while(|(a, b)| a == b)
            .count()
    });
    // The suffix is matched on the prefix-stripped remainders, so the two
    // strips cannot overlap.
    let rests: Vec<&[char]> = chars.iter().map(|c| &c[prefix..]).collect();
    let suffix = rests[1..].iter().fold(rests[0].len(), |len, c| {
        rests[0][rests[0].len() - len..]
            .iter()
            .rev()
            .zip(c.iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
    });
    rests
        .iter()
        .zip(commands)
        .map(|(rest, cmd)| {
            let name: String = rest[..rest.len() - suffix].iter().collect();
            let name = name.trim();
            if name.is_empty() {
                cmd.clone()
            } else {
                name.to_string()
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(cmds: &[&str]) -> Vec<String> {
        let cmds: Vec<String> = cmds.iter().map(|s| s.to_string()).collect();
        auto_names(&cmds)
    }

    #[test]
    fn auto_names_strip_common_prefix_and_suffix() {
        assert_eq!(
            names(&["sort --merge f", "sort --quick f"]),
            ["merge", "quick"]
        );
        assert_eq!(names(&["echo 1", "echo 2", "echo 3"]), ["1", "2", "3"]);
    }

    #[test]
    fn auto_names_fall_back_to_the_command() {
        // A single command (or identical ones) is entirely common.
        assert_eq!(names(&["echo hi"]), ["echo hi"]);
        assert_eq!(names(&["a", "a"]), ["a", "a"]);
    }

    #[test]
    fn auto_names_prefix_and_suffix_never_overlap() {
        // The prefix "a" consumes all of "a"; the suffix must not re-consume it.
        assert_eq!(names(&["aa", "a"]), ["a", "a"]);
    }
}
