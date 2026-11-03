mod app;
mod branch;
mod config;
mod exec;
mod focus;
mod merge;
mod model;
mod pr;
mod preview;
mod refresh;
mod source;
mod status;
mod term;
mod ui;
mod worktree;

const USAGE: &str = "cmux-cc-monitor — Claude Code sessions in cmux as tiles (hjkl move, Enter focus, p preview, i settings, q quit)";

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("cmux-cc-monitor {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
    let (config, load_error) = config::Config::load_checked(&config::default_path());
    term::install_panic_hook();
    let mut guard = term::TerminalGuard::new()?;
    app::run(&mut guard.terminal, config, load_error)
}
