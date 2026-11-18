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
            Ok(cfg) if cfg.home_port != 0 => home::server::serve(cfg.home_port, cfg.clean_settings()),
            Ok(_) => {
                eprintln!("decidle serve: config の home_port に 0 は使えない");
                1
            }
            Err(e) => {
                eprintln!("decidle serve: {e}");
                1
            }
        },
        Some("clean") => clean(&args[2..]),
        _ => {
            eprintln!("usage: decidle stop | decidle render <report.json> | decidle extract [--projects-dir DIR] | decidle event | decidle serve | decidle clean [--dry-run]");
            1
        }
    };
    std::process::exit(code);
}

/// `decidle clean [--dry-run]`: deletes old reports once, as `decidle serve` does every day.
fn clean(args: &[String]) -> i32 {
    let dry_run = match args {
        [] => false,
        [a] if a == "--dry-run" => true,
        _ => {
            eprintln!("usage: decidle clean [--dry-run]");
            return 1;
        }
    };
    let cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("decidle clean: {e}");
            return 1;
        }
    };
    let settings = cfg.clean_settings();
    if settings.off() {
        eprintln!("decidle clean: config の clean_after_days と clean_unanswered_after_days が 0 なので消さない");
        return 0;
    }
    match home::clean::run(&home::events::events_path(), &settings, dry_run) {
        Ok(o) => {
            home::clean::report(&o, dry_run);
            0
        }
        Err(e) => {
            eprintln!("decidle clean: {e}");
            1
        }
    }
}
