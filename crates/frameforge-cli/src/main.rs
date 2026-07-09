mod args;
mod catalog;
mod command;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    command::run(env::args_os())
}
