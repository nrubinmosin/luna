// Frame instrumentation for the gate "a burst renders with no frame over 16 ms
// and no dropped output". A frame's time (wall time on the UI thread, so it
// counts any preemption: it is not thread CPU time) runs from the start of the window
// root's render (TermView::render; the pane is the window's root view) to the
// end of the terminal element's paint, both on the UI thread with Instant: it
// covers render, layout, prepaint (the copy out of the Term under its lock and
// building every quad and glyph) and paint, not gpui's scene submission and
// present after it. The interval is from one frame's render start to the
// next, counted while output is pending: the frame shows bytes that were
// parsed no later than one vsync (16.7 ms) after the previous frame started,
// so a gap in the output (between bursts) is not counted as a slow frame.
// Latency is from the end of the oldest parse a frame shows to that frame's
// render start; a frame whose latency is over one vsync (16.667 ms) is a late
// frame: output was waiting at a vsync and no render started for it. Lock wait
// is how long the frame's one Term lock (in prepaint) waited, lock hold how
// long it held it. None of it includes the GPU present after paint.
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct FrameStats {
    pub frame_start: Option<Instant>,
    prev_start: Option<Instant>,
    /// Only frames while `recording` are kept (the feed's run).
    pub recording: bool,
    pub frame_ms: Vec<f64>,
    pub interval_ms: Vec<f64>,
    pub latency_ms: Vec<f64>,
    pub lock_wait_ms: Vec<f64>,
    pub lock_hold_ms: Vec<f64>,
    pub all_frames: u64,
}

/// One vsync at 60 Hz: the bound for a late frame.
pub const VSYNC_MS: f64 = 1000.0 / 60.0;

impl FrameStats {
    pub fn render_start(&mut self, now: Instant) {
        self.prev_start = self.frame_start.or(self.prev_start);
        self.frame_start = Some(now);
    }

    /// This frame shows output whose oldest parse finished at `parsed_at`.
    pub fn output_shown(&mut self, parsed_at: Instant) {
        let Some(start) = self.frame_start else { return };
        if !self.recording {
            return;
        }
        self.latency_ms.push(ms(start.saturating_duration_since(parsed_at)));
        if let Some(prev) = self.prev_start {
            if parsed_at <= prev + Duration::from_secs_f64(VSYNC_MS / 1000.0) {
                self.interval_ms.push(ms(start - prev));
            }
        }
    }

    pub fn lock(&mut self, wait: Duration, hold: Duration) {
        if self.recording {
            self.lock_wait_ms.push(ms(wait));
            self.lock_hold_ms.push(ms(hold));
        }
    }

    pub fn paint_end(&mut self, now: Instant) {
        if let Some(start) = self.frame_start {
            self.prev_start = Some(start);
            self.all_frames += 1;
            if self.recording {
                self.frame_ms.push(ms(now - start));
            }
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub struct Dist {
    pub count: usize,
    pub max: f64,
    pub p99: f64,
    pub p50: f64,
    pub mean: f64,
}

pub fn dist(v: &[f64]) -> Dist {
    if v.is_empty() {
        return Dist { count: 0, max: 0.0, p99: 0.0, p50: 0.0, mean: 0.0 };
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    Dist { count: s.len(), max: *s.last().unwrap(), p99: q(0.99), p50: q(0.5), mean: s.iter().sum::<f64>() / s.len() as f64 }
}

pub fn dist_json(v: &[f64], over: &[f64]) -> String {
    let d = dist(v);
    let mut s = format!(
        "{{\"count\":{},\"max\":{:.3},\"p99\":{:.3},\"p50\":{:.3},\"mean\":{:.3}",
        d.count, d.max, d.p99, d.p50, d.mean
    );
    for &o in over {
        s += &format!(",\"over_{}ms\":{}", o, v.iter().filter(|&&x| x > o).count());
    }
    s + "}"
}

pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out + "\""
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let d = dist(&v);
        assert_eq!((d.count, d.max, d.p50), (100, 100.0, 51.0));
        assert_eq!(d.p99, 99.0);
        assert!(dist_json(&v, &[16.0]).contains("\"over_16ms\":84"));
    }
}
