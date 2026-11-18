mod check;
mod config;
mod extract;
mod gate;
mod render;
mod transcript;
mod util;

use decidle::home;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("stop") => {
            gate::run_stop();
            0
        }
        Some("extract") => extract::run(&args[2..]),
        Some("render") => gate::run_render(&args[2..]),
        Some("event") => home::events::run_hook(),
        Some("serve") => match config::Config::load() {
            Ok(cfg) if cfg.home_port != 0 => home::server::serve(cfg.home_port),
            Ok(_) => {
                eprintln!("decidle serve: config の home_port に 0 は使えない");
                1
            }
            Err(e) => {
                eprintln!("decidle serve: {e}");
                1
            }
        },
        _ => {
            eprintln!("usage: decidle stop | decidle render <report.json> | decidle extract [--projects-dir DIR] | decidle event | decidle serve");
            1
        }
    };
    std::process::exit(code);
}
