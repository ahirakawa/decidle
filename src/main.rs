mod check;
mod config;
mod extract;
mod gate;
mod hint;
mod render;
mod transcript;
mod util;

use decidle::home;
use decidle::i18n::{self, Lang};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("stop") => {
            gate::run_stop();
            0
        }
        Some("hint") => {
            hint::run();
            0
        }
        Some("extract") => extract::run(&args[2..]),
        Some("render") => gate::run_render(&args[2..]),
        Some("event") => {
            // The hook never fails, so a config it cannot read falls back to the default language.
            let lang = config::Config::load().map(|c| c.language).unwrap_or_else(|e| {
                eprintln!("decidle event: {e}");
                Lang::default()
            });
            home::events::run_hook(lang)
        }
        Some("serve") => match config::Config::load() {
            Ok(cfg) if cfg.home_port != 0 => home::server::serve(cfg.home_port, cfg.clean_settings(), cfg.language),
            Ok(_) => {
                eprintln!("{}", i18n::t(i18n::current(), "cli.port_zero"));
                1
            }
            Err(e) => {
                eprintln!("decidle serve: {e}");
                1
            }
        },
        Some("clean") => clean(&args[2..]),
        _ => {
            eprintln!("usage: decidle stop | decidle hint | decidle render <report.json> | decidle extract [--projects-dir DIR] | decidle event | decidle serve | decidle clean [--dry-run]");
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
        eprintln!("{}", i18n::t(cfg.language, "cli.clean_off"));
        return 0;
    }
    match home::clean::run(&home::events::events_path(), &settings, dry_run) {
        Ok(o) => {
            home::clean::report(&o, dry_run, cfg.language);
            0
        }
        Err(e) => {
            eprintln!("decidle clean: {e}");
            1
        }
    }
}
