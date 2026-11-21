//! `luna-window-shots [--size WxH] [--scale PCT] OUT [FILTER]`: the window's
//! demo scenes as PNGs in OUT, those whose name contains FILTER when one is
//! given, at WxH and at the window scale PCT (one of its steps) when asked
//! (src/shots.rs).
//! `luna-window-shots --bench SECS OUT.json`: the terminal's burst bench
//! (src/term/bench), its summary in OUT.json.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        eprintln!("usage: luna-window-shots [--size WxH] [--scale PCT] OUT [FILTER] | --bench SECS OUT.json");
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
    fn usage() -> ! {
        eprintln!("usage: luna-window-shots [--size WxH] [--scale PCT] OUT [FILTER]");
        std::process::exit(2);
    }
    let (mut size, mut scale, mut next) = (None, None, Some(first));
    let out = loop {
        match next.take() {
            Some(flag) if flag == "--size" => {
                let s = args.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
                });
                size = Some(s.unwrap_or_else(|| usage()));
            }
            Some(flag) if flag == "--scale" => {
                scale = Some(args.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or_else(|| usage()));
            }
            Some(out) => break out,
            None => usage(),
        }
        next = args.next();
    };
    let out = std::path::PathBuf::from(out);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("shots: cannot make {}: {e}", out.display());
        std::process::exit(2);
    }
    luna_window::shots(out, args.next().unwrap_or_default(), size, scale);
}
