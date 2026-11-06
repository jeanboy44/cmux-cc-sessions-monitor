mod app;
mod branch;
mod config;
mod exec;
mod focus;
mod spirit;
mod merge;
mod model;
mod pr;
mod preview;
mod refresh;
mod source;
mod status;
mod storage;
mod term;
mod ui;
mod worktree;

const USAGE: &str = "cmux-magigent — Claude Code sessions in cmux as tiles (hjkl move, Enter focus, s status, m memory, p preview, i settings, q quit)\n\n메모리 CLI 도구:\n  cmux-magigent memory [list|show|search|timeline|obs|exec|conv|sync|stats]";

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("cmux-magigent {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("memory") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            return storage::run_cli(&args);
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
    let (config, load_error) = config::Config::load_checked(&config::load_path());
    term::install_panic_hook();
    let mut guard = term::TerminalGuard::new()?;
    app::run(&mut guard.terminal, config, load_error)
}
