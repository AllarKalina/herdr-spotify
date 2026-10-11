use std::process::ExitCode;

fn main() -> ExitCode {
    match herdr_spotify::cli::run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
