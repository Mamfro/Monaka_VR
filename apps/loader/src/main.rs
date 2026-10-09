//! `monaka_loader start|stop PID DLL`: attaches a producer DLL to a running game and calls its
//! `MonakaStart` or `MonakaStop`. The work is `monaka_launch::attach`; this is its command line, kept for
//! the PowerShell launchers and tools.

use monaka_launch::attach;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let usage = || {
        eprintln!("Usage: monaka_loader start|stop PID absolute-path-to-producer.dll");
        ExitCode::from(1)
    };
    if args.len() != 4 || !matches!(args[1].as_str(), "start" | "stop") {
        return usage();
    }
    let Some(pid) = args[2].parse::<u32>().ok().filter(|&p| p != 0) else { return usage() };
    let dll = Path::new(&args[3]);
    let result = if args[1] == "start" { attach::start(pid, dll) } else { attach::stop(pid, dll) };
    match result {
        Ok(outcome) => {
            for note in &outcome.notes {
                println!("{note}");
            }
            println!("{} returned {}. Logs are beside the DLL.", args[1], outcome.code);
            if outcome.code == 0 { ExitCode::SUCCESS } else { ExitCode::from(2) }
        }
        Err(failure) => {
            eprintln!("{failure}");
            ExitCode::from(3)
        }
    }
}
