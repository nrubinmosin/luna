//! The synthetic burst feed: a Rust port of the generator in
//! test-build/perf/js/tb-feed.js (the one tb-uptime.ps1 plays into the page's
//! xterm panes), deterministic from a seed, as the GPUI spike measured with
//! it. Frames alternate by burst between a Claude Code style live region
//! (erased line by line and rewritten, with a shimmering spinner line and
//! finished blocks printed above it) and a Codex style inline viewport
//! (history inserted through a scroll region, the viewport repainted by
//! cursor positioning). Blocks are code, diffs, prose with wide characters,
//! boxes with block elements, and on every sixth burst truecolor-heavy lines
//! (a fresh 24-bit fg and bg on every character).

/// mulberry32, bit for bit as tb-feed.js has it.
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed)
    }

    pub fn float(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let s = self.0;
        let mut t = (s ^ (s >> 15)).wrapping_mul(1 | s);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.float() * n as f64) as usize
    }

    fn pick<'a>(&mut self, a: &[&'a str]) -> &'a str {
        a[self.below(a.len())]
    }
}

const R: &str = "\x1b[0m";
const WORDS: &[&str] = &[
    "const",
    "let",
    "return",
    "await",
    "function",
    "import",
    "from",
    "if",
    "else",
    "match",
    "impl",
    "pub",
    "fn",
    "self",
    "Some(x)",
    "None",
    "Ok(())",
    "=>",
    "{",
    "}",
    "(",
    ")",
    "[i]",
    "&&",
    "||",
    "terminal",
    "renderRows",
    "atlas",
    "pages.push",
    "\"string\"",
    "0x1f",
    "42",
    "true",
    "null",
];
const SYNTAX: &[[u32; 3]] = &[
    [198, 120, 221],
    [97, 175, 239],
    [152, 195, 121],
    [229, 192, 123],
    [224, 108, 117],
    [86, 182, 194],
    [209, 154, 102],
    [171, 178, 191],
    [127, 132, 142],
    [190, 80, 70],
];
const PROSE: &[&str] = &[
    "The",
    "renderer",
    "keeps",
    "a",
    "texture",
    "atlas",
    "per",
    "font",
    "config,",
    "and",
    "every",
    "new",
    "glyph",
    "color",
    "pair",
    "adds",
    "to",
    "it.",
    "**bold**",
    "`code`",
    "résumé",
    "naïve",
    "Ünïcödé",
];
const WIDE: &[&str] = &["✅", "🚀", "⚠️", "🔥", "完成", "テスト", "한국어", "中文字符", "日本語", "🎉", "📦", "✨", "→", "…", "★"]; // lang-check: allow
const SPIN: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽"];
const TC: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789{}()[]<>=+-*/";

fn fg(c: [u32; 3]) -> String {
    format!("\x1b[38;2;{};{};{}m", c[0], c[1], c[2])
}

fn bg(c: [u32; 3]) -> String {
    format!("\x1b[48;2;{};{};{}m", c[0], c[1], c[2])
}

/// Math.round(a + (b - a) * t).
fn lerp(a: f64, b: f64, t: f64) -> u32 {
    (a + (b - a) * t + 0.5).floor() as u32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Claude,
    Codex,
}

pub struct Generator {
    rng: Rng,
    frame: u64,
    live: usize,
    pub kind: Kind,
    pub entropy: f64,
}

impl Generator {
    pub fn new(seed: u32) -> Self {
        Generator { rng: Rng::new(seed), frame: 0, live: 0, kind: Kind::Claude, entropy: 0.3 }
    }

    /// Switches style; the next frame starts on a fresh line.
    pub fn set_kind(&mut self, kind: Kind) -> &'static str {
        if self.kind == kind {
            return "";
        }
        self.kind = kind;
        self.live = 0;
        "\x1b[r\x1b[999;1H\r\n"
    }

    /// One frame for a `w` x `h` terminal.
    pub fn frame(&mut self, w: usize, h: usize) -> String {
        let s = match self.kind {
            Kind::Claude => self.claude_frame(w),
            Kind::Codex => self.codex_frame(w, h),
        };
        self.frame += 1;
        s
    }

    fn rgb(&mut self) -> [u32; 3] {
        [self.rng.below(256) as u32, self.rng.below(256) as u32, self.rng.below(256) as u32]
    }

    fn token(&mut self, entropy: f64) -> String {
        let c = if self.rng.float() < entropy { self.rgb() } else { SYNTAX[self.rng.below(SYNTAX.len())] };
        fg(c) + self.rng.pick(WORDS)
    }

    fn code_line(&mut self, w: usize, entropy: f64) -> String {
        let mut s = format!("\x1b[38;5;{}m{:>4} {R}", 236 + self.rng.below(8), self.rng.below(999));
        while s.len() < w * 3 {
            s += &self.token(entropy);
            s.push(' ');
        }
        s + R
    }

    fn diff_line(&mut self, w: usize, entropy: f64) -> String {
        let add = self.rng.float() < 0.5;
        let base = if add {
            [self.rng.below(8) as u32 + 20, 50 + self.rng.below(8) as u32, 30]
        } else {
            [60 + self.rng.below(8) as u32, 20 + self.rng.below(8) as u32, 24]
        };
        let hi = if add { [30, 90 + self.rng.below(10) as u32, 45] } else { [110 + self.rng.below(10) as u32, 32, 36] };
        let mut s = bg(base) + &if add { fg([140, 220, 140]) + "+ " } else { fg([230, 140, 140]) + "- " };
        let mut n = 2;
        while n + 12 < w {
            let word = self.token(entropy) + " ";
            if self.rng.float() < 0.15 {
                s += &(bg(hi) + &word + &bg(base));
            } else {
                s += &word;
            }
            n += 8;
        }
        s + "\x1b[K" + R
    }

    fn prose_line(&mut self) -> String {
        let mut s = String::from("  ");
        for _ in 0..10 {
            let word = if self.rng.float() < 0.15 { self.rng.pick(WIDE) } else { self.rng.pick(PROSE) };
            s += word;
            s.push(' ');
        }
        let bold = if self.rng.float() < 0.2 { "\x1b[1m" } else { "" };
        let italic = if self.rng.float() < 0.2 { "\x1b[3m" } else { "" };
        format!("{bold}{italic}{s}{R}")
    }

    fn tc_line(&mut self, w: usize) -> String {
        let mut s = String::with_capacity(w * 40);
        for _ in 0..w.saturating_sub(1) {
            let (f, b) = (self.rgb(), self.rgb());
            s += &fg(f);
            s += &bg(b);
            s.push(TC[self.rng.below(TC.len())] as char);
        }
        s + R
    }

    fn block(&mut self, w: usize, entropy: f64) -> Vec<String> {
        if entropy >= 2.0 {
            return (0..6).map(|_| self.tc_line(w)).collect();
        }
        let mut lines = Vec::new();
        match self.rng.below(5) {
            0 => {
                lines.push(format!("{}⏺{R} \x1b[1mBash{R}(cargo test --lib {})", fg([140, 220, 140]), self.rng.below(99)));
                lines.push(format!("  ⎿  {}running {} tests{R}", fg([150, 150, 150]), self.rng.below(500)));
            }
            1 => {
                lines.push(format!("{}⏺{R} \x1b[1mUpdate{R}(src/features/panes/terminals.ts)", fg([229, 192, 123])));
                for _ in 0..6 {
                    lines.push(self.diff_line(w, entropy));
                }
            }
            2 => {
                for _ in 0..6 {
                    lines.push(self.code_line(w, entropy));
                }
            }
            3 => {
                for _ in 0..3 {
                    lines.push(self.prose_line());
                }
            }
            _ => {
                let iw = (w.saturating_sub(2)).min(60);
                lines.push(format!("\x1b[38;5;{}m╭{}╮{R}", self.rng.below(256), "─".repeat(iw)));
                let c = self.rng.below(256);
                let (a, b) = (self.rng.pick(WIDE), self.rng.pick(PROSE));
                lines.push(format!("\x1b[38;5;{c}m│{R} {a} {b} {}", "░▒▓█".repeat(3)));
                lines.push(format!("\x1b[38;5;{}m╰{}╯{R}", self.rng.below(256), "─".repeat(iw)));
            }
        }
        lines
    }

    fn claude_frame(&mut self, w: usize) -> String {
        let f = self.frame;
        let mut s = String::new();
        for i in 0..self.live {
            s += "\x1b[2K";
            if i + 1 < self.live {
                s += "\x1b[1A";
            }
        }
        s.push('\r');
        if f.is_multiple_of(25) {
            for l in self.block(w, self.entropy) {
                s += &l;
                s += "\r\n";
            }
        }
        let tokens = (f as f64 * 3.7 + 0.5).floor() as u64;
        let msg: Vec<char> =
            format!("{} Pondering… ({}s · ↑ {tokens} tokens · esc to interrupt)", SPIN[f as usize % SPIN.len()], f / 10).chars().collect();
        let center = (f as f64 * 0.7) % (msg.len() as f64 + 10.0) - 5.0;
        let mut line = String::new();
        for (i, ch) in msg.iter().enumerate() {
            let t = (1.0 - (i as f64 - center).abs() / 4.0).max(0.0);
            line += &fg([lerp(215.0, 255.0, t), lerp(119.0, 200.0, t), lerp(87.0, 170.0, t)]);
            line.push(*ch);
        }
        let iw = w.saturating_sub(2).max(10);
        let typed = (f % 20) as usize;
        let out = [
            line + R,
            String::new(),
            format!("\x1b[38;5;244m╭{}╮{R}", "─".repeat(iw)),
            format!("\x1b[38;5;244m│{R} > {}{}\x1b[38;5;244m│{R}", "x".repeat(typed), " ".repeat(iw.saturating_sub(3 + typed))),
            format!("\x1b[38;5;244m╰{}╯{R}", "─".repeat(iw)),
            format!("  \x1b[2m⏵⏵ accept edits on · {}% context{R}", f % 100),
        ];
        s += &out.join("\r\n");
        self.live = out.len();
        s
    }

    fn codex_frame(&mut self, w: usize, h: usize) -> String {
        let f = self.frame;
        let vp = 6;
        let top = h.saturating_sub(vp).max(1);
        let mut s = String::new();
        if f.is_multiple_of(20) {
            for l in self.block(w, self.entropy) {
                s += &format!("\x1b[1;{top}r\x1b[{top};1H\r\n{l}\x1b[r");
            }
        }
        let msg: Vec<char> = format!("Working ({}s • esc to interrupt)", f / 10).chars().collect();
        let band = (f as f64 * 0.37) % (msg.len() as f64 + 8.0) - 4.0;
        let mut line = String::from("• ");
        for (i, ch) in msg.iter().enumerate() {
            let d = (i as f64 - band).abs();
            let t = if d < 3.0 { 0.5 * (1.0 + (std::f64::consts::PI * d / 3.0).cos()) } else { 0.0 };
            let v = lerp(128.0, 255.0, t);
            line += &fg([v, v, v]);
            line.push(*ch);
        }
        let ask = "Ask Codex to do anything";
        let n = (f % 25) as usize;
        s += &format!("\x1b[{};1H\x1b[2K{line}{R}", top + 1);
        s += &format!("\x1b[{};1H\x1b[48;5;236m\x1b[2K› {}{R}", top + 3, &ask[..n.min(ask.len())]);
        s += &format!("\x1b[{};3H\x1b[2m{}% context left · ? for shortcuts{R}", top + 5, 100 - (f % 100));
        s += &format!("\x1b[{};{}H", top + 3, 3 + n);
        s
    }
}

/// The burst schedule over a run: bursts of `BURST` seconds with `IDLE`
/// seconds of silence after each, alternating Claude and Codex style; every
/// sixth burst is the truecolor-heavy one (tb-uptime.ps1's -TrueColorEvery 6).
pub const BURST: f64 = 4.0;
pub const IDLE: f64 = 1.0;

pub struct Phase {
    pub burst: usize,
    pub active: bool,
    pub kind: Kind,
    pub entropy: f64,
}

pub fn phase(t: f64) -> Phase {
    let cycle = BURST + IDLE;
    let burst = (t / cycle) as usize;
    let active = t - burst as f64 * cycle < BURST;
    let kind = if burst.is_multiple_of(2) { Kind::Claude } else { Kind::Codex };
    let entropy = if burst % 6 == 5 { 2.0 } else { 0.3 };
    Phase { burst, active, kind, entropy }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_is_deterministic() {
        // Not checked against tb-feed.js itself: there is no Node here to run it.
        let mut r = Rng::new(12345);
        let a = r.float();
        let b = r.float();
        assert!((0.0..1.0).contains(&a) && (0.0..1.0).contains(&b) && a != b);
        let mut again = Rng::new(12345);
        assert_eq!(again.float(), a);
    }

    #[test]
    fn deterministic_frames() {
        let run = || {
            let mut g = Generator::new(7);
            (0..60).map(|_| g.frame(100, 30)).collect::<String>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn every_sixth_burst_is_truecolor() {
        assert_eq!(phase(0.5).entropy, 0.3);
        assert_eq!(phase(5.0 * 5.0 + 0.1).entropy, 2.0);
        assert!(!phase(4.5).active);
        assert_eq!(phase(5.5).kind, Kind::Codex);
    }
}
