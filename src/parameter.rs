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

/// Fewest decimals (at least `floor`, capped) that render every `--parameter-step-n`
/// value back to itself: a computed step like 0.25 needs 2 decimals to stay exact,
/// while a non-terminating one (1/3) rounds at the cap and passes the rounded value.
fn scan_decimals(values: &[f64], floor: usize) -> usize {
    const CAP: usize = 6;
    for d in floor..CAP {
        let faithful = values.iter().all(|&v| {
            let parsed: f64 = format!("{v:.d$}").parse().unwrap();
            (parsed - v).abs() <= v.abs().max(1.0) * 1e-9
        });
        if faithful {
            return d;
        }
    }
    CAP
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
                // A bare word is more likely a stray argument (`-L l 1 10`)
                // than a one-value list, so require at least one comma.
                if !pair[1].contains(',') {
                    bail!(
                        "--parameter-list {}: no comma in VALUES '{v}'; if you want \
                         only one value you can simply add a trailing comma ('{v},')",
                        pair[0],
                        v = pair[1],
                    );
                }
                let list = pair[1].strip_suffix(',').unwrap_or(&pair[1]);
                let values: Vec<String> = list.split(',').map(str::to_string).collect();
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
        if min > max {
            bail!("--parameter-scan {}: MIN {min} > MAX {max}", triple[0]);
        }
        // The bounds' own decimals are the floor for the generated labels.
        let bound_d = decimals(&triple[1]).max(decimals(&triple[2]));
        let values = match cli.parameter_step_n {
            // NUM equal steps: NUM+1 evenly spaced values, endpoints exact.
            Some(n) => {
                if n == 0 {
                    bail!("--parameter-step-n must be at least 1");
                }
                if min >= max {
                    bail!(
                        "--parameter-scan {}: MIN must be below MAX with --parameter-step-n",
                        triple[0]
                    );
                }
                if n + 1 > MAX_COMMANDS {
                    bail!("--parameter-scan {} yields too many values", triple[0]);
                }
                // Endpoints are pinned exactly; interior points are linear or
                // geometric between them.
                let points: Vec<f64> = if cli.parameter_step_log {
                    if min <= 0.0 {
                        bail!(
                            "--parameter-scan {}: MIN must be positive with --parameter-step-log",
                            triple[0]
                        );
                    }
                    let ratio = (max / min).powf(1.0 / n as f64);
                    (0..=n)
                        .map(|i| match i {
                            0 => min,
                            i if i == n => max,
                            i => min * ratio.powi(i as i32),
                        })
                        .collect()
                } else {
                    let step = (max - min) / n as f64;
                    (0..=n)
                        .map(|i| if i == n { max } else { min + i as f64 * step })
                        .collect()
                };
                let d = scan_decimals(&points, bound_d);
                points.iter().map(|v| format!("{v:.d$}")).collect()
            }
            // Fixed step size (default 1.0).
            None => {
                let step = cli.parameter_step_size;
                if step <= 0.0 {
                    bail!("--parameter-step-size must be positive");
                }
                let d = bound_d.max(decimals(&step.to_string()));
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
                values
            }
        };
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
    fn scan_step_n_integer() {
        let c = cli(&["-P", "s", "3", "10", "--parameter-step-n", "7", "echo {s}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(
            cmds,
            [
                "echo 3", "echo 4", "echo 5", "echo 6", "echo 7", "echo 8", "echo 9", "echo 10"
            ]
        );
    }

    #[test]
    fn scan_step_n_fractional_keeps_exact_values() {
        // step = 0.25, so labels need two decimals to stay faithful.
        let c = cli(&["-P", "d", "0", "1", "--parameter-step-n", "4", "echo {d}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(
            cmds,
            [
                "echo 0.00",
                "echo 0.25",
                "echo 0.50",
                "echo 0.75",
                "echo 1.00"
            ]
        );
    }

    #[test]
    fn scan_step_n_endpoints_are_exact() {
        // A non-dividing range still starts at MIN and ends at MAX exactly.
        let c = cli(&["-P", "n", "1", "10", "--parameter-step-n", "3", "echo {n}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds.first().unwrap(), "echo 1");
        assert_eq!(cmds.last().unwrap(), "echo 10");
        assert_eq!(cmds.len(), 4);
    }

    #[test]
    fn scan_step_n_errors() {
        assert!(expand(&cli(&["-P", "n", "1", "3", "--parameter-step-n", "0", "a"])).is_err());
        // MIN == MAX has no steps to divide.
        assert!(expand(&cli(&["-P", "n", "5", "5", "--parameter-step-n", "2", "a"])).is_err());
    }

    #[test]
    fn scan_step_log_is_geometric() {
        let c = cli(&[
            "-P",
            "s",
            "1",
            "1000",
            "--parameter-step-n",
            "3",
            "--parameter-step-log",
            "echo {s}",
        ]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1", "echo 10", "echo 100", "echo 1000"]);
    }

    #[test]
    fn scan_step_log_needs_positive_min() {
        let c = cli(&[
            "-P",
            "s",
            "0",
            "100",
            "--parameter-step-n",
            "2",
            "--parameter-step-log",
            "a",
        ]);
        assert!(expand(&c).is_err());
    }

    #[test]
    fn scan_step_log_requires_step_n() {
        let argv = [
            "megafine",
            "-P",
            "s",
            "1",
            "100",
            "--parameter-step-log",
            "a",
        ];
        assert!(Cli::try_parse_from(argv).is_err());
    }

    #[test]
    fn scan_step_n_conflicts_with_step_size() {
        let argv = [
            "megafine",
            "-P",
            "n",
            "1",
            "3",
            "--parameter-step-n",
            "2",
            "--parameter-step-size",
            "1",
            "a",
        ];
        assert!(Cli::try_parse_from(argv).is_err());
    }

    #[test]
    fn single_value_needs_trailing_comma() {
        let err = expand(&cli(&["-L", "a", "1", "echo {a}"])).unwrap_err();
        assert!(err.to_string().contains("trailing comma"), "{err}");
    }

    #[test]
    fn trailing_comma_allows_single_value() {
        let c = cli(&["-L", "a", "1,", "echo {a}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1"]);
        // Also tolerated on a multi-value list.
        let c = cli(&["-L", "a", "1,2,", "echo {a}"]);
        let (cmds, _) = expand(&c).unwrap();
        assert_eq!(cmds, ["echo 1", "echo 2"]);
    }

    #[test]
    fn names_are_substituted() {
        let c = cli(&["-L", "a", "1,2", "echo {a}", "-n", "run {a}"]);
        let (_, names) = expand(&c).unwrap();
        assert_eq!(names, ["run 1", "run 2"]);
    }

    #[test]
    fn unknown_placeholder_errors() {
        let c = cli(&["-L", "a", "1,", "echo {b}"]);
        assert!(expand(&c).is_err());
    }

    #[test]
    fn shell_braces_pass_through() {
        let c = cli(&["-L", "a", "1,", "awk '{print $1}' f{a}"]);
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
        assert!(expand(&cli(&["-L", "a", "1,", "-L", "a", "2,", "a"])).is_err());
    }
}
