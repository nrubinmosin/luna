// Diagnostics for the macOS run (ci/gpui-spike.yml), usable anywhere:
//
//   --log LEVEL     a logger on stderr (error|warn|info|debug|trace), so gpui's
//                   own warnings show (a missing feature, a skipped frame)
//   --diag PATH     a JSON line to PATH at start (GPU, display, fonts), then one
//                   a second: app and window state, display-link ticks,
//                   renders, notifies, feeder ticks, foreground timer lateness
//   --no-app-nap    macOS: holds an NSProcessInfo activity (UserInitiated |
//                   LatencyCritical) for the whole run, so App Nap and timer
//                   coalescing do not apply
//   --activate      macOS: activates the app at start and again 1 s later
//                   (ignored elsewhere: on Windows nothing of the spike's may
//                   take the foreground, AGENTS.md)
//
// The per-second line tells a platform stall from ours: `ticks` counts the
// window's frame callbacks (one per display-link update that reached gpui,
// whether or not it drew), `renders` the root view's renders, `notifies` the
// terminal's redraw requests, `feeder` the feed thread's 16 ms ticks and
// `fg_late_max_ms` how late a 50 ms foreground timer fired.

use std::{
    io::Write as _,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, App, Window, font};

use crate::idle;

#[derive(Default)]
pub struct DiagArgs {
    pub log: Option<log::LevelFilter>,
    pub frames: Option<PathBuf>,
    pub no_app_nap: bool,
    pub activate: bool,
}

impl DiagArgs {
    /// Takes `arg` (and its value from `it`) if it is one of these flags.
    pub fn parse(&mut self, arg: &str, it: &mut impl Iterator<Item = String>) -> bool {
        match arg {
            "--log" => self.log = it.next().and_then(|s| s.parse().ok()),
            "--diag" => self.frames = it.next().map(Into::into),
            "--no-app-nap" => self.no_app_nap = true,
            "--activate" => self.activate = true,
            _ => return false,
        }
        true
    }
}

/// Display-link updates that reached the window (its next-frame callbacks).
pub static TICKS: AtomicUsize = AtomicUsize::new(0);
/// The terminal's redraw requests (cx.notify on new output).
pub static NOTIFIES: AtomicUsize = AtomicUsize::new(0);
static FEEDER_TICKS: AtomicUsize = AtomicUsize::new(0);
static FEEDER_MAX_US: AtomicU64 = AtomicU64::new(0);
static FG_LATE_MAX_US: AtomicU64 = AtomicU64::new(0);

/// One feeder tick of `ms` (start to start), from the feed thread.
pub fn feeder_tick(ms: f64) {
    FEEDER_TICKS.fetch_add(1, Ordering::Relaxed);
    FEEDER_MAX_US.fetch_max((ms * 1000.0) as u64, Ordering::Relaxed);
}

struct StderrLog {
    start: Instant,
}

impl log::Log for StderrLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr(),
                "[{:9.3}s {:5} {}] {}",
                self.start.elapsed().as_secs_f64(),
                record.level(),
                record.target(),
                record.args()
            );
        }
    }
    fn flush(&self) {}
}

/// Before the app starts: the logger and the App Nap opt-out.
pub fn before_app(args: &DiagArgs) {
    if let Some(level) = args.log {
        if log::set_boxed_logger(Box::new(StderrLog { start: Instant::now() })).is_ok() {
            log::set_max_level(level);
        }
    }
    if args.no_app_nap {
        #[cfg(target_os = "macos")]
        mac::begin_activity();
        #[cfg(not(target_os = "macos"))]
        eprintln!("spike: --no-app-nap is macOS only; ignored");
    }
}

/// Once the windows are open: activation and the per-second log, whose frame
/// callbacks are counted on `window`.
pub fn start(args: &DiagArgs, window: AnyWindowHandle, cx: &mut App) {
    if args.activate {
        #[cfg(target_os = "macos")]
        {
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                cx.update(|cx| cx.activate(true));
            })
            .detach();
        }
        #[cfg(not(target_os = "macos"))]
        eprintln!("spike: --activate is macOS only; ignored");
    }
    let Some(path) = args.frames.clone() else {
        return;
    };
    let _ = window.update(cx, |_, window, _| chain_ticks(window));

    // How late a foreground timer fires: the main thread's wakeups.
    cx.spawn(async move |cx| {
        let period = Duration::from_millis(50);
        loop {
            let asked = Instant::now();
            cx.background_executor().timer(period).await;
            let late = asked.elapsed().saturating_sub(period);
            FG_LATE_MAX_US.fetch_max(late.as_micros() as u64, Ordering::Relaxed);
        }
    })
    .detach();

    let first = format!(
        "{{\"start\":true,\"no_app_nap\":{},\"activate\":{},\"platform\":{},\"fonts\":{}}}",
        args.no_app_nap,
        args.activate,
        platform_json(),
        fonts_json(cx)
    );
    let mut file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("spike: --diag {}: {e}", path.display());
            return;
        }
    };
    let _ = writeln!(file, "{first}");
    let started = Instant::now();
    cx.spawn(async move |cx| {
        let mut last = [0usize; 5];
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let now = [
                TICKS.load(Ordering::Relaxed),
                idle::MAIN_RENDERS.load(Ordering::Relaxed),
                idle::TERM_RENDERS.load(Ordering::Relaxed),
                idle::EDITOR_RENDERS.load(Ordering::Relaxed),
                NOTIFIES.load(Ordering::Relaxed),
            ];
            let d: Vec<usize> = now.iter().zip(last).map(|(n, l)| n - l).collect();
            last = now;
            let gpui_active = window
                .update(cx, |_, window, _| window.is_window_active())
                .ok();
            let line = format!(
                "{{\"t\":{:.3},\"ticks\":{},\"renders\":{{\"main\":{},\"term\":{},\"editor\":{}}},\"notifies\":{},\"feeder\":{{\"ticks\":{},\"max_ms\":{:.1}}},\"fg_late_max_ms\":{:.1},\"gpui_window_active\":{},\"mac\":{}}}",
                started.elapsed().as_secs_f64(),
                d[0],
                d[1],
                d[2],
                d[3],
                d[4],
                FEEDER_TICKS.swap(0, Ordering::Relaxed),
                FEEDER_MAX_US.swap(0, Ordering::Relaxed) as f64 / 1000.0,
                FG_LATE_MAX_US.swap(0, Ordering::Relaxed) as f64 / 1000.0,
                gpui_active.map_or("null".into(), |a| a.to_string()),
                state_json(),
            );
            let _ = writeln!(file, "{line}");
        }
    })
    .detach();
}

/// A next-frame callback that registers the next one: it runs once per frame
/// request the platform delivers (gpui runs these whether or not it draws).
fn chain_ticks(window: &mut Window) {
    window.on_next_frame(|window, _| {
        TICKS.fetch_add(1, Ordering::Relaxed);
        chain_ticks(window);
    });
}

/// The fonts the spike and gpui-component ask for, and what each resolves to
/// (resolve_font falls back to a default family when one is missing).
fn fonts_json(cx: &App) -> String {
    let ts = cx.text_system();
    let mut families: Vec<String> = vec![crate::term::element::FAMILY.into(), ".SystemUIFont".into()];
    if let Some(theme) = cx.try_global::<gpui_component::Theme>() {
        families.push(theme.font_family.to_string());
        families.push(theme.mono_font_family.to_string());
    }
    let resolved: Vec<String> = families
        .iter()
        .map(|f| {
            let id = ts.resolve_font(&font(f.clone()));
            let got = ts.get_font_for_id(id).map(|f| f.family.to_string()).unwrap_or_default();
            format!("{{\"asked\":{},\"font_id\":{},\"family\":{}}}", json_str(f), id.0, json_str(&got))
        })
        .collect();
    format!(
        "{{\"all_font_names\":{},\"resolved\":[{}]}}",
        ts.all_font_names().len(),
        resolved.join(",")
    )
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

#[cfg(target_os = "macos")]
fn platform_json() -> String {
    mac::platform_json()
}
#[cfg(not(target_os = "macos"))]
fn platform_json() -> String {
    "null".into()
}

#[cfg(target_os = "macos")]
fn state_json() -> String {
    mac::state_json()
}
#[cfg(not(target_os = "macos"))]
fn state_json() -> String {
    "null".into()
}

/// AppKit, Foundation and Metal through objc2's msg_send!, read on the main
/// thread (gpui's foreground).
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, c_char};

    use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};

    use super::json_str;

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }

    fn string(obj: Option<&AnyObject>) -> String {
        let Some(obj) = obj else { return String::new() };
        let p: *const c_char = unsafe { msg_send![obj, UTF8String] };
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    }

    fn process_info() -> Retained<AnyObject> {
        unsafe { msg_send![class!(NSProcessInfo), processInfo] }
    }

    /// NSActivityUserInitiated | NSActivityLatencyCritical, held until exit.
    pub fn begin_activity() {
        const USER_INITIATED: u64 = 0x00FF_FFFF | (1 << 20);
        const LATENCY_CRITICAL: u64 = 0xFF_0000_0000;
        let reason: Retained<AnyObject> = unsafe {
            msg_send![class!(NSString), stringWithUTF8String: c"gpui spike frame-gap diagnostics".as_ptr()]
        };
        let token: Option<Retained<AnyObject>> = unsafe {
            msg_send![&*process_info(), beginActivityWithOptions: USER_INITIATED | LATENCY_CRITICAL, reason: &*reason]
        };
        eprintln!("spike: --no-app-nap: activity {}", if token.is_some() { "begun" } else { "refused" });
        // Ended by process exit only.
        std::mem::forget(token);
    }

    pub fn platform_json() -> String {
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        let device: Option<Retained<AnyObject>> = unsafe { Retained::from_raw(device) };
        let (name, headless, low_power) = match &device {
            Some(d) => {
                let name: Option<Retained<AnyObject>> = unsafe { msg_send![&**d, name] };
                let headless: bool = unsafe { msg_send![&**d, isHeadless] };
                let low_power: bool = unsafe { msg_send![&**d, isLowPower] };
                (string(name.as_deref()), headless, low_power)
            }
            None => (String::new(), false, false),
        };
        let screen: Option<Retained<AnyObject>> = unsafe { msg_send![class!(NSScreen), mainScreen] };
        let (fps, scale) = match &screen {
            Some(s) => {
                let fps: isize = unsafe { msg_send![&**s, maximumFramesPerSecond] };
                let scale: f64 = unsafe { msg_send![&**s, backingScaleFactor] };
                (fps, scale)
            }
            None => (0, 0.0),
        };
        let screens: Retained<AnyObject> = unsafe { msg_send![class!(NSScreen), screens] };
        let screen_count: usize = unsafe { msg_send![&*screens, count] };
        format!(
            "{{\"metal_device\":{},\"metal_headless\":{headless},\"metal_low_power\":{low_power},\"screen_max_fps\":{fps},\"screen_scale\":{scale},\"screens\":{screen_count}}}",
            json_str(&name)
        )
    }

    pub fn state_json() -> String {
        let app: Retained<AnyObject> = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        let active: bool = unsafe { msg_send![&*app, isActive] };
        let hidden: bool = unsafe { msg_send![&*app, isHidden] };
        let policy: isize = unsafe { msg_send![&*app, activationPolicy] };
        // NSApplicationOcclusionState: Visible = 1 << 1.
        let app_occlusion: usize = unsafe { msg_send![&*app, occlusionState] };
        let workspace: Retained<AnyObject> = unsafe { msg_send![class!(NSWorkspace), sharedWorkspace] };
        let front: Option<Retained<AnyObject>> = unsafe { msg_send![&*workspace, frontmostApplication] };
        let front_name = match &front {
            Some(f) => {
                let n: Option<Retained<AnyObject>> = unsafe { msg_send![&**f, localizedName] };
                string(n.as_deref())
            }
            None => String::new(),
        };
        let windows: Retained<AnyObject> = unsafe { msg_send![&*app, windows] };
        let count: usize = unsafe { msg_send![&*windows, count] };
        let mut list = Vec::new();
        for i in 0..count {
            let w: Retained<AnyObject> = unsafe { msg_send![&*windows, objectAtIndex: i] };
            let title: Option<Retained<AnyObject>> = unsafe { msg_send![&*w, title] };
            // NSWindowOcclusionState: Visible = 1 << 1.
            let occlusion: usize = unsafe { msg_send![&*w, occlusionState] };
            let visible: bool = unsafe { msg_send![&*w, isVisible] };
            let key: bool = unsafe { msg_send![&*w, isKeyWindow] };
            let on_space: bool = unsafe { msg_send![&*w, isOnActiveSpace] };
            list.push(format!(
                "{{\"title\":{},\"occlusion\":{occlusion},\"occlusion_visible\":{},\"visible\":{visible},\"key\":{key},\"on_active_space\":{on_space}}}",
                json_str(&string(title.as_deref())),
                occlusion & 2 != 0
            ));
        }
        let info = process_info();
        let thermal: isize = unsafe { msg_send![&*info, thermalState] };
        let low_power: bool = unsafe { msg_send![&*info, isLowPowerModeEnabled] };
        format!(
            "{{\"app_active\":{active},\"app_hidden\":{hidden},\"activation_policy\":{policy},\"app_occlusion_visible\":{},\"frontmost\":{},\"thermal\":{thermal},\"low_power\":{low_power},\"windows\":[{}]}}",
            app_occlusion & 2 != 0,
            json_str(&front_name),
            list.join(",")
        )
    }
}
