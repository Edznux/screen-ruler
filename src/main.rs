//! screen-ruler: measure distances between UI edges on any screen.
//!
//! At launch every monitor is captured, analysed into an edge map, and covered
//! by its own fullscreen window showing that frozen screenshot. Measurements
//! are then made against the static edge map rather than the live screen, which
//! is what makes the readings stable while the cursor moves.

mod capture;
mod cli;
mod clipboard;
mod color;
mod edges;
mod export;
mod geometry;
mod image;
mod measure;
mod png;
mod regions;
mod state;
mod surface;
mod ui;
mod window;

use std::process::ExitCode;

use state::RulerState;
use surface::Desktop;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        cli::Parsed::Run(options) => options,
        cli::Parsed::Help => {
            print!("{}", cli::usage());
            return ExitCode::SUCCESS;
        }
        cli::Parsed::Version => {
            println!("screen-ruler {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        cli::Parsed::Error(message) => {
            eprintln!("screen-ruler: {message}");
            eprintln!("Try 'screen-ruler --help' for more information.");
            return ExitCode::FAILURE;
        }
    };

    let captured = match capture::capture_all() {
        Ok(captured) => captured,
        Err(e) => {
            eprintln!("screen-ruler: {e}");
            eprintln!("{}", capture::permission_hint());
            return ExitCode::FAILURE;
        }
    };

    let (low, high) = edges::sensitivity_to_thresholds(options.sensitivity);
    let desktop = Desktop::build(captured, low, high);
    if desktop.is_empty() {
        eprintln!("screen-ruler: no display could be analysed");
        return ExitCode::FAILURE;
    }

    let state = RulerState::new(options.sensitivity, options.debug_edges);
    if let Err(e) = window::App::new(desktop, state).run() {
        eprintln!("screen-ruler: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
