use hls_native_engine::{load_job, run_job, EXIT_ERROR};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(job_path) = flag_value(&args, "--job") else {
        eprintln!("usage: hls-native-engine --job <job.json>");
        return ExitCode::from(EXIT_ERROR as u8);
    };
    match load_job(&PathBuf::from(job_path)).and_then(|job| run_job(&job)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
