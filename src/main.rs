use std::ffi::OsStr;

const USAGE: &str = "usage: herdr-command-palette <open|run>";

fn exit_with_usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next();
    if args.next().is_some() {
        exit_with_usage();
    }

    let result = match mode.as_deref() {
        Some(mode) if mode == OsStr::new("open") => herdr_command_palette::app::run_open(),
        Some(mode) if mode == OsStr::new("run") => herdr_command_palette::app::run_palette(),
        _ => exit_with_usage(),
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
