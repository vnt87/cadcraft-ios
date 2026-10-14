# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-13 · **Change:** minor (iPadOS fork section; earlier: zoom and pan regenerate in the background) · **Target:** Autodesk AutoCAD 2027 (Mac and Windows)

The hardware AutoCAD uses and how CADCraft compares, per platform. Work items:
[gaps.md](gaps.md#hardware).

**Headline (estimated): hardware ≈ 45%.** 2D GPU drawing, HiDPI and trackpads are covered on
every platform. Missing: 3D mice, tablets/digitizers, GPU-accelerated 3D (there is no 3D), and
any deliberate multi-monitor handling. Remaining: **≈ 15–25 Opus 5.5 hours**, plus a human with
the devices to test 3D mice and tablets.

Measured from the source on 2026-10-10 (`crates/ui-egui/src/gpu.rs`, `canvas.rs`,
`apps/cadcraft/src/graphics.rs`, `apps/cadcraft-web`); AutoCAD's side from Autodesk's system
requirements and graphics-performance documentation. Nothing was benchmarked in this pass.

| Feature | AutoCAD 2027 | CADCraft macOS | CADCraft Windows | CADCraft Linux / FreeBSD | CADCraft web |
|---|---|---|---|---|---|
| GPU canvas for 2D | yes (hardware acceleration, GPU smoothing, high-quality geometry) | wgpu on Metal | wgpu on DirectX 12 by default (`WGPU_BACKEND` overrides; GL avoided after AMD crashes, #31) | wgpu on Vulkan (or GL) | WebGPU, WebGL2 fallback |
| CPU fallback | software mode | yes (`tiny-skia` rasteriser) | yes | yes | — |
| GPU for 3D (shaded visual styles, ray-traced rendering) | yes | no (no 3D) | no | no | no |
| Low-latency presentation | yes | vsync or low-latency via `CADCRAFT_VSYNC` | same | no vsync queue so the crosshair keeps up (#183) | browser-controlled |
| HiDPI / Retina, display scaling | yes | yes (egui points per pixel) | yes; a Windows 11 DPI fix is queued upstream (PR #110) | yes | yes |
| Multi-monitor (palettes on other screens, per-monitor DPI) | yes | not handled deliberately; one window | same | same | — |
| Trackpad pan, pinch zoom | yes (Mac) | yes, with trackpad vs wheel detection (#58) | wheel only (precision touchpad untested) | wheel; delay reported on Linux Mint (#25) | browser events |
| Magic Mouse | yes | treated as a trackpad | — | — | — |
| Wheel mouse, middle-button pan | yes | yes | yes | yes | yes |
| 3D mouse (3Dconnexion SpaceMouse) | yes | **no** | **no** | **no** | no |
| Pen tablet / digitizer (TABLET command, calibration) | yes (Windows) | **no** | **no** | **no** | no |
| Screen magnifier following the cursor | yes | — | breaks (#39; PR #187 adds a system-cursor option) | — | — |
| Plotters and printers (PC3 devices, system printers) | yes | **no**: PDF/PNG/SVG files only; Print asks where to save the PDF (#26) | same | same | PDF bytes shown instead of a download (#134) |

## Performance on hardware (internal numbers only)

From the earlier roadmap (measured by the agents that built it, 2026-10-06, `cadcraft-cli perf`):
200,000 entities pick in 0.001 ms, about 4 ms after an edit (incremental R-tree), and the GPU
canvas draws a frame in about 4 ms. Nothing has been compared with AutoCAD on the same machine,
and 1M-entity drawings are untested. A 10 MB DWG took about two minutes to fail (#57).

Zoom and pan no longer freeze the window (2026-10-13). On a hatched 12k-entity architect's DWG
(about 340k display primitives) each zoom step used to block the UI for 2–4.5 s while the
display list, its vertex data and its GPU buffers were rebuilt. These rebuilds now run on a
worker thread while the previous list is drawn, so the longest stall is 26–64 ms (M-series Mac).
Edits still rebuild on the UI thread (about 1.8 s on that drawing): per-entity caching is the
next step.

## iPadOS (fork)

Only what has been demonstrated. On the iPad Air 11-inch (M3) Simulator with iPadOS 26.2 (2026-10-13): the
Debug app builds for Apple Silicon, installs, launches and draws its interface in portrait inside the safe
area. On a physical iPad Air 13-inch (M3), reported by the maintainer (2026-10-14): the app installs and runs;
with a hardware keyboard (GameController key codes, because winit delivers no Enter, arrows or shortcuts and,
with no text field focused, no text either) typing into the command line works, including Shift, `@`,
Backspace and hold-to-repeat, and Cmd+O opens the Files picker and Cmd+S saves. File pickers use
`Services::begin_pick`/`poll_pick` (answered on a later frame) over UIKit's document picker; host tests cover
that flow and the runner's storage and change tracking. Not demonstrated or not wired up: opening and saving
a drawing end to end, touch, multitasking, landscape. Written and unit-tested on the host but not yet
exercised on a device: the system text clipboard and links through UIKit, the saved panel layout, and Apple
Pencil hover (the pointer follows the floating tip) with pressure, tilt and roll read per sample; no CAD
tool uses pressure or tilt yet, and the Pencil double-tap and squeeze are not read.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | minor | iPadOS fork: hardware keyboard typing, Backspace, repeat and Cmd+O/Cmd+S confirmed on a physical iPad; Apple Pencil hover and pose added (host-tested, not yet exercised) |
| 2026-10-14 | minor | iPadOS fork: hardware keyboard, clipboard, links and saved layout added (host-tested, not yet exercised on a device) |
| 2026-10-13 | minor | iPadOS fork section: simulator launch and interface drawn only; everything else listed as not demonstrated |
| 2026-10-13 | minor | Zoom and pan regenerate in the background: 2–4.5 s stalls down to 26–64 ms on a hatched DWG |
| 2026-10-10 | major | First hardware parity doc: GPU backends per platform, input devices, displays, plotters |
