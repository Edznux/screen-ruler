//! Command-line parsing.
//!
//! Hand-rolled rather than pulling in an argument-parsing crate: the interface
//! is four flags, and every dependency avoided is one less thing to keep
//! building on three platforms.

use crate::edges;

/// Parsed command-line options.
#[derive(Debug, PartialEq)]
pub struct Options {
    /// Starting sensitivity, 0..100.
    pub sensitivity: f32,
    /// Keep the edge map visible for alignment debugging.
    pub debug_edges: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sensitivity: edges::DEFAULT_SENSITIVITY,
            debug_edges: false,
        }
    }
}

/// What the caller should do after parsing.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    Run(Options),
    /// Print usage and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
    /// Report a usage error and exit non-zero.
    Error(String),
}

pub const USAGE: &str = "\
screen-ruler - measure distances between UI edges on any screen

USAGE:
    screen-ruler [OPTIONS]

OPTIONS:
    --sensitivity N        Edge-detection sensitivity, 0-100 (default: 85).
                           Higher finds more edges.
    --threshold-low N      Lower Canny threshold, 0-255. Overrides --sensitivity.
    --threshold-high N     Upper Canny threshold, 0-255. Overrides --sensitivity.
    --debug-edge-overlay   Keep the detected edge map visible.
    -h, --help             Show this help.
    -V, --version          Show the version.

MODES (press 1-6)
    1  Crosshair          Measure to the nearest edges around the cursor
    2  Drag rectangle     Drag a rectangle, snapping to edges
    3  Container          Detect the enclosing UI container
    4  Shrink-to-fit      Drag, then tighten onto the content inside
    5  Color picker       Sample a colour, optionally averaged
    6  Point distance     Measure between two points

KEYS
    Tab            Toggle session mode (persistent annotations)
    Click          Copy the measurement and quit / place an annotation
    Enter          Copy the current selection and quit
    Ctrl+C         Copy the measurement / export annotations as Markdown
    Ctrl+Z         Undo annotation        Ctrl+Shift+Z  Redo annotation
    Ctrl+Shift+C   Drag a region to copy it with annotations
    Wheel          Adjust the active mode's control
    ? or H         Toggle the shortcut overlay
    Esc / Q        Quit
";

/// Parses arguments, excluding the program name.
pub fn parse<I, S>(args: I) -> Parsed
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
    let mut options = Options::default();
    let mut low: Option<u16> = None;
    let mut high: Option<u16> = None;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        // Accept both `--flag value` and `--flag=value`.
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (arg, None),
        };

        let mut take_value = |what: &str| -> Result<String, String> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };

        match name {
            "-h" | "--help" => return Parsed::Help,
            "-V" | "--version" => return Parsed::Version,
            "--debug-edge-overlay" => options.debug_edges = true,
            "--sensitivity" => match take_value("--sensitivity") {
                Ok(value) => match value.parse::<f32>() {
                    Ok(parsed) if (0.0..=100.0).contains(&parsed) => options.sensitivity = parsed,
                    Ok(parsed) => {
                        return Parsed::Error(format!("--sensitivity must be 0-100, got {parsed}"))
                    }
                    Err(_) => {
                        return Parsed::Error(format!("--sensitivity expects a number, got '{value}'"))
                    }
                },
                Err(e) => return Parsed::Error(e),
            },
            "--threshold-low" | "--threshold-high" => {
                let value = match take_value(name) {
                    Ok(value) => value,
                    Err(e) => return Parsed::Error(e),
                };
                match value.parse::<u16>() {
                    Ok(parsed) if parsed <= 255 => {
                        if name == "--threshold-low" {
                            low = Some(parsed);
                        } else {
                            high = Some(parsed);
                        }
                    }
                    _ => {
                        return Parsed::Error(format!("{name} must be 0-255, got '{value}'"));
                    }
                }
            }
            other => return Parsed::Error(format!("unknown option '{other}'")),
        }

        index += 1;
    }

    // Explicit thresholds win, and map back onto the slider so the UI agrees
    // with what was asked for on the command line.
    if low.is_some() || high.is_some() {
        let (default_low, default_high) = edges::sensitivity_to_thresholds(options.sensitivity);
        let low = low.unwrap_or(default_low);
        let high = high.unwrap_or(default_high);
        if high <= low {
            return Parsed::Error(format!(
                "--threshold-high ({high}) must be greater than --threshold-low ({low})"
            ));
        }
        options.sensitivity = edges::thresholds_to_sensitivity(low, high);
    }

    Parsed::Run(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Options {
        match parse(args) {
            Parsed::Run(options) => options,
            other => panic!("expected a run, got {other:?}"),
        }
    }

    fn error(args: &[&str]) -> String {
        match parse(args) {
            Parsed::Error(message) => message,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_uses_the_defaults() {
        let options = run(&[]);
        assert_eq!(options.sensitivity, edges::DEFAULT_SENSITIVITY);
        assert!(!options.debug_edges);
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse(["--help"]), Parsed::Help);
        assert_eq!(parse(["-h"]), Parsed::Help);
        assert_eq!(parse(["--version"]), Parsed::Version);
        assert_eq!(parse(["-V"]), Parsed::Version);
        // They win even alongside other arguments.
        assert_eq!(parse(["--sensitivity", "50", "--help"]), Parsed::Help);
    }

    #[test]
    fn sensitivity_accepts_both_argument_forms() {
        assert_eq!(run(&["--sensitivity", "42"]).sensitivity, 42.0);
        assert_eq!(run(&["--sensitivity=42"]).sensitivity, 42.0);
    }

    #[test]
    fn the_debug_flag_is_recognised() {
        assert!(run(&["--debug-edge-overlay"]).debug_edges);
    }

    #[test]
    fn explicit_thresholds_map_back_onto_the_slider() {
        // The values the Python implementation shipped as defaults.
        let options = run(&["--threshold-low", "16", "--threshold-high", "54"]);
        assert!(
            (options.sensitivity - edges::DEFAULT_SENSITIVITY).abs() < 1.0,
            "got {}",
            options.sensitivity
        );
    }

    #[test]
    fn one_threshold_alone_is_combined_with_the_sensitivity_default() {
        let options = run(&["--threshold-low", "5"]);
        assert!(options.sensitivity > 0.0);
    }

    #[test]
    fn out_of_range_and_malformed_values_are_rejected() {
        assert!(error(&["--sensitivity", "150"]).contains("0-100"));
        assert!(error(&["--sensitivity", "abc"]).contains("expects a number"));
        assert!(error(&["--threshold-low", "999"]).contains("0-255"));
        assert!(error(&["--sensitivity"]).contains("needs a value"));
    }

    #[test]
    fn crossed_thresholds_are_rejected() {
        let message = error(&["--threshold-low", "100", "--threshold-high", "20"]);
        assert!(message.contains("must be greater than"), "{message}");
    }

    #[test]
    fn unknown_options_are_reported_by_name() {
        assert!(error(&["--wat"]).contains("--wat"));
    }
}
