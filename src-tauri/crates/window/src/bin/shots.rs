//! `luna-window-shots OUT [FILTER]`: the window's demo scenes as PNGs in OUT,
//! those whose name contains FILTER when one is given (src/shots.rs).
//! `luna-window-shots --bench SECS OUT.json`: the terminal's burst bench
//! (src/term/bench), its summary in OUT.json.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        eprintln!("usage: luna-window-shots OUT [FILTER] | --bench SECS OUT.json");
        std::process::exit(2);
    };
    if first == "--bench" {
        let (Some(secs), Some(out)) = (args.next().and_then(|s| s.parse::<f64>().ok()), args.next()) else {
            eprintln!("usage: luna-window-shots --bench SECS OUT.json");
            std::process::exit(2);
        };
        luna_window::bench(out.into(), secs);
        return;
    }
    let out = std::path::PathBuf::from(first);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("shots: cannot make {}: {e}", out.display());
        std::process::exit(2);
    }
    luna_window::shots(out, args.next().unwrap_or_default());
}
