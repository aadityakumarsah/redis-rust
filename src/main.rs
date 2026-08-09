use std::process::ExitCode;

use redis_server::{config::Config, server};

fn main() -> ExitCode {
    let config = match Config::from_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("Try 'redis-server --help'");
            return ExitCode::FAILURE;
        }
    };
    match server::run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fatal: {e}");
            ExitCode::FAILURE
        }
    }
}
