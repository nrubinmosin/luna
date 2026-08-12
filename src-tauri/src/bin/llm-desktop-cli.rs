//! Console companion for the GUI app.
//!
//! The app itself is a `windows_subsystem = "windows"` binary: it has no
//! console, and a shell does not wait for it — so it can never report whether
//! anything worked. This binary is an ordinary console program, so it blocks
//! until the app answers and exits with a meaningful code.
//!
//! Shares the request format with the app by compiling the same module.

#[path = "../cli.rs"]
mod cli;

const USAGE: &str = "\
llm-desktop-cli --new-chat --folder <path> [options]

  --folder <path>     Project directory for the session (required)
  --prompt <text>     First message to send once the session is up
  --model <name>      Opus | Fable | Haiku | Sonnet      (default: Sonnet)
  --effort <level>    low | medium | high | xhigh | max  (default: medium)
  --account <name>    Claude account to run on           (default: the first)
  --worktree          Run in an isolated git worktree    (default)
  --no-worktree       Run directly in the folder

Exits 0 once llm-desktop confirms the session started.
The app must already be running.";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let Some(req) = cli::parse_new_chat(&args) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };

    match cli::submit_and_wait(&req) {
        Ok(()) => println!("session started in {}", req.folder),
        Err(e) => {
            eprintln!("llm-desktop: {e}");
            std::process::exit(1);
        }
    }
}
