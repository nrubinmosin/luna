# The macOS job's summary.txt (ci/gpui-spike.yml): the facts a run is read by,
# from the files in OUT (the first argument), on stdout. Missing files show as
# None; nothing here fails. ci/COPY.md, "Reading summary.txt", says what the
# lines mean.
import glob
import json
import os
import re
import statistics
import sys

out = sys.argv[1] if len(sys.argv) > 1 else "out"


def read(name):
    try:
        with open(os.path.join(out, name), encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return None


def load(name):
    try:
        return json.loads(read(name) or "")
    except ValueError:
        return None


def diag(name):
    rows = []
    for line in (read(name) or "").splitlines():
        try:
            rows.append(json.loads(line))
        except ValueError:
            pass
    return rows


def get(d, *keys):
    for k in keys:
        d = d.get(k) if isinstance(d, dict) else None
    return d


lines = ["versions: " + " ".join((read("versions.txt") or "").split())]

tree = read("cargo-tree.txt") or ""
font_kit = bool(re.search(r"^zed-font-kit ", tree, re.M))
lines.append(f"text: zed-font-kit in the macOS graph: {'yes' if font_kit else 'NO'}")
logs = "".join(
    open(p, encoding="utf-8", errors="replace").read()
    for p in sorted(glob.glob(os.path.join(out, "*stderr.txt")))
)
warned = "without the `font-kit` feature" in logs
lines.append(f"text: gpui's 'compiled without the font-kit feature' warning: {'YES' if warned else 'no'}")
m = re.search(r"label_pixels=(\d+)", read("inapp.txt") or "")
if m:
    n = int(m.group(1))
    lines.append(f"text: in-app label pixels (main window, render_to_image): {n} -> {'TEXT DRAWN' if n > 50 else 'NO TEXT'}")
else:
    lines.append("text: in-app label pixels: not measured (did the shot step fail?)")

start = next((r for r in diag("term-diag.jsonl") if r.get("start")), None)
if start:
    lines.append("gpu/display: " + json.dumps(start.get("platform")))
    fonts = start.get("fonts") or {}
    resolved = ", ".join(f"{r.get('asked')}->{r.get('family')}" for r in fonts.get("resolved", []))
    lines.append(f"fonts: all_font_names={fonts.get('all_font_names')}; {resolved}")
lines.append(
    f"log: 'skipped a frame drawn outside its display link' lines: {logs.count('skipped a frame drawn outside')}; "
    f"ERROR lines: {len(re.findall(r' ERROR ', logs))}"
)

lines.append(
    "frames, term burst:20, while the feed ran: run: frames | interval max ms | latency p50/max ms | "
    "feeder tick p50/mean/max ms | s app active/s | s window occluded | display ticks/s min/median | "
    "fg timer late max ms"
)
for name in ("term", "term-nonap", "term-active", "term-both"):
    st = load(f"{name}-stats.json") or {}
    rows = [r for r in diag(f"{name}-diag.jsonl") if "t" in r and get(r, "feeder", "ticks")]
    macs = [r.get("mac") or {} for r in rows]
    active = sum(1 for m in macs if m.get("app_active"))
    occluded = sum(1 for m in macs if any(not w.get("occlusion_visible") for w in m.get("windows", [])))
    ticks = [r.get("ticks", 0) for r in rows]
    late = max((r.get("fg_late_max_ms", 0) for r in rows), default=None)
    ft = st.get("feeder_tick_ms") or {}
    lines.append(
        f"  {name}: {st.get('frames')} | {get(st, 'interval_ms', 'max')} | "
        f"{get(st, 'latency_ms', 'p50')}/{get(st, 'latency_ms', 'max')} | "
        f"{ft.get('p50')}/{ft.get('mean')}/{ft.get('max')} | {active}/{len(rows)} | {occluded} | "
        f"{min(ticks) if ticks else None}/{statistics.median(ticks) if ticks else None} | {late}"
    )
    osa = read(f"{name}-osascript.txt")
    if osa is not None:
        lines.append(f"    osascript: {osa.strip()[:200] or 'ok, no output'}")

text = "\n".join(lines) + "\n"
sys.stdout.write(text)
if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
        f.write("```\n" + text + "```\n")
