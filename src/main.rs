#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    zop::run(std::env::args_os())
}
