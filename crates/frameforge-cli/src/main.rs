use std::env;
use std::process::ExitCode;

use frameforge_core::VERSION;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("-h") | Some("--help") | Some("help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") | Some("version") => {
            println!("ff {VERSION}");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("error: unknown command '{command}'");
            eprintln!("run 'ff --help' for usage");
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!(
        "FrameForge {VERSION}

Usage:
  ff --help
  ff --version

FrameForge is being bootstrapped as a safe Rust media pipeline toolkit.
Codec, filter, and validation commands will be added as the crates mature."
    );
}
