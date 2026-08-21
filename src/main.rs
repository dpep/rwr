//! rwr — Ruby structural search and rewrite.

use std::process::ExitCode;

fn main() -> ExitCode {
    rwr::cli::run()
}
