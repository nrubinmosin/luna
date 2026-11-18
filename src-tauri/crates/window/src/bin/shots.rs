//! `luna-window-shots OUT [FILTER]`: the window's demo scenes as PNGs in OUT,
//! those whose name contains FILTER when one is given (src/shots.rs).

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        eprintln!("usage: luna-window-shots OUT [FILTER]");
        std::process::exit(2);
    };
    let out = std::path::PathBuf::from(out);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("shots: cannot make {}: {e}", out.display());
        std::process::exit(2);
    }
    luna_window::shots(out, args.next().unwrap_or_default());
}
