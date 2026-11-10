mod check;
mod config;
mod extract;
mod gate;
mod render;
mod transcript;
mod util;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("stop") => {
            gate::run_stop();
            0
        }
        Some("extract") => extract::run(&args[2..]),
        Some("render") => gate::run_render(&args[2..]),
        _ => {
            eprintln!("usage: report-gate stop | report-gate render <report.json> | report-gate extract [--projects-dir DIR]");
            1
        }
    };
    std::process::exit(code);
}
