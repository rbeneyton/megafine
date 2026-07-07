use anyhow::{Context, Result, bail};

use crate::cli::Cli;

/// Ceiling on the expanded command count, so a typo'd scan range fails fast
/// instead of allocating millions of commands.
const MAX_COMMANDS: usize = 10_000;

/// One `{name}` parameter axis and the values it takes.
struct Axis {
    name: String,
    values: Vec<String>,
}

/// Number of decimals in the textual form of a bound or step, so generated
/// scan values render like the user wrote them (never 0.30000000000000004).
fn decimals(s: &str) -> usize {
    s.split_once('.').map_or(0, |(_, frac)| frac.len())
}

/// Collect the -L/-P axes (clap guarantees the pair/triple grouping).
fn axes(cli: &Cli) -> Result<Vec<Axis>> {
    let mut axes = Vec::new();
    for pair in cli.parameter_list.chunks_exact(2) {
        // `@FILE` reads one value per line (trimmed, blank lines skipped).
        let values: Vec<String> = match pair[1].strip_prefix('@') {
            Some(path) => {
                let text = std::fs::read_to_string(path).with_context(|| {
                    format!(
                        "--parameter-list {}: cannot read values from '{path}'",
                        pair[0]
                    )
                })?;
                let values: Vec<String> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                if values.is_empty() {
                    bail!("--parameter-list {}: file '{path}' has no values", pair[0]);
                }
                values
            }
            None => {
                let values: Vec<String> = pair[1].split(',').map(str::to_string).collect();
                if values.iter().any(String::is_empty) {
                    bail!("--parameter-list {} has an empty value", pair[0]);
                }
                values
            }
        };
        axes.push(Axis {
            name: pair[0].clone(),
            values,
        });
    }
    for triple in cli.parameter_scan.chunks_exact(3) {
        let parse = |s: &String| -> Result<f64> {
            s.parse()
                .with_context(|| format!("--parameter-scan {}: '{s}' is not a number", triple[0]))
        };
        let (min, max) = (parse(&triple[1])?, parse(&triple[2])?);
        let step = cli.parameter_step_size;
        if step <= 0.0 {
            bail!("--parameter-step-size must be positive");
        }
        if min > max {
            bail!("--parameter-scan {}: MIN {min} > MAX {max}", triple[0]);
        }
        let d = decimals(&triple[1])
            .max(decimals(&triple[2]))
            .max(decimals(&step.to_string()));
        let mut values = Vec::new();
        let mut v = min;
        // Absorb the accumulated float error so MAX itself is included.
        while v <= max + step * 1e-9 {
            values.push(format!("{v:.d$}"));
            v += step;
            if values.len() > MAX_COMMANDS {
                bail!("--parameter-scan {} yields too many values", triple[0]);
            }
        }
        axes.push(Axis {
            name: triple[0].clone(),
            values,
        });
    }
    for (i, a) in axes.iter().enumerate() {
        if axes[..i].iter().any(|b| b.name == a.name) {
            bail!("parameter '{}' is defined twice", a.name);
        }
    }
    Ok(axes)
}

/// The first `{identifier}` token left in `text`, if any. Only bare
/// identifiers count, so shell braces like `awk '{print $1}'` pass through.
fn placeholder(text: &str) -> Option<&str> {
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let end = rest.find('}')?;
        let token = &rest[..end];
        if !token.is_empty()
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Some(token);
        }
        rest = &rest[end + 1..];
    }
    None
}

/// Expand the -L/-P parameters: every command (and its -n display name) is
/// repeated once per point of the axes' cross product, with each `{name}`
/// replaced by the point's value. Commands vary slowest, then the axes in
/// definition order.
pub fn expand(cli: &Cli) -> Result<(Vec<String>, Vec<String>)> {
    let axes = axes(cli)?;
    let mut combos: Vec<Vec<&str>> = vec![Vec::new()];
    for axis in &axes {
        combos = combos
            .iter()
            .flat_map(|combo| {
                axis.values.iter().map(|v| {
                    let mut combo = combo.clone();
                    combo.push(v.as_str());
                    combo
                })
            })
            .collect();
    }
    if cli.commands.len() * combos.len() > MAX_COMMANDS {
        bail!(
            "parameters expand to {} commands (max {MAX_COMMANDS})",
            cli.commands.len() * combos.len()
        );
    }

    let substitute = |text: &str, combo: &[&str]| {
        let mut out = text.to_string();
        for (axis, value) in axes.iter().zip(combo) {
            out = out.replace(&format!("{{{}}}", axis.name), value);
        }
        out
    };
    let commands: Vec<String> = cli
        .commands
        .iter()
        .flat_map(|cmd| combos.iter().map(|combo| substitute(cmd, combo)))
        .collect();
    let names: Vec<String> = cli
        .command_name
        .iter()
        .flat_map(|name| combos.iter().map(|combo| substitute(name, combo)))
        .collect();

    for cmd in &commands {
        if let Some(token) = placeholder(cmd) {
            bail!("unknown parameter '{{{token}}}' in command '{cmd}'");
        }
    }
    Ok((commands, names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let argv = std::iter::once("megafine").chain(args.iter().copied());
        Cli::try_parse_from(argv).expect("args should parse at the clap level")
    }

    #[test]
    fn list_cross_product() {
        let c = cli(&["-L", "a", "1,2", "-L", "b", "x,y", "echo {a}{b}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1x", "echo 1y", "echo 2x", "echo 2y"]);
    }

    #[test]
    fn commands_vary_slowest() {
        let c = cli(&["-L", "a", "1,2", "echo {a}", "true {a}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1", "echo 2", "true 1", "true 2"]);
    }

    #[test]
    fn scan_integers() {
        let c = cli(&["-P", "n", "1", "3", "echo {n}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1", "echo 2", "echo 3"]);
    }

    #[test]
    fn scan_decimal_step() {
        let c = cli(&[
            "-P",
            "d",
            "0",
            "1",
            "--parameter-step-size",
            "0.5",
            "echo {d}",
        ]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 0.0", "echo 0.5", "echo 1.0"]);
    }

    #[test]
    fn list_from_file() {
        let path = std::env::temp_dir().join(format!("megafine-params-{}", std::process::id()));
        std::fs::write(&path, "1\n\n  2  \n").unwrap();
        let arg = format!("@{}", path.display());
        let c = cli(&["-L", "a", &arg, "echo {a}"]);
        let result = expand(&c);
        std::fs::remove_file(&path).unwrap();
        let (cmds, _) = result.unwrap();
        assert_eq!(cmds, ["echo 1", "echo 2"]);
    }

    #[test]
    fn list_file_errors() {
        assert!(expand(&cli(&["-L", "a", "@/nonexistent/megafine-params", "a"])).is_err());
        let path = std::env::temp_dir().join(format!("megafine-empty-{}", std::process::id()));
        std::fs::write(&path, "\n  \n").unwrap();
        let arg = format!("@{}", path.display());
        let result = expand(&cli(&["-L", "a", &arg, "a"]));
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn names_are_substituted() {
        let c = cli(&["-L", "a", "1,2", "echo {a}", "-n", "run {a}"]);
        let (_, names) = expand(&c).unwrap();
        assert_eq!(names, ["run 1", "run 2"]);
    }

    #[test]
    fn unknown_placeholder_errors() {
        let c = cli(&["-L", "a", "1", "echo {b}"]);
        assert!(expand(&c).is_err());
    }

    #[test]
    fn shell_braces_pass_through() {
        let c = cli(&["-L", "a", "1", "awk '{print $1}' f{a}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["awk '{print $1}' f1"]);
    }

    #[test]
    fn invalid_ranges_error() {
        assert!(expand(&cli(&["-P", "n", "3", "1", "a"])).is_err());
        assert!(expand(&cli(&["-P", "n", "x", "1", "a"])).is_err());
        assert!(
            expand(&cli(&[
                "-P",
                "n",
                "1",
                "2",
                "--parameter-step-size",
                "0",
                "a"
            ]))
            .is_err()
        );
        assert!(expand(&cli(&["-L", "a", "1,,2", "a"])).is_err());
        assert!(expand(&cli(&["-L", "a", "1", "-L", "a", "2", "a"])).is_err());
    }
}
