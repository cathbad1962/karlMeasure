# Measure

A desktop application for measuring areas on calibrated PDF drawings.

Open a drawing, tell it what a known distance on the page really is, and trace
areas over it with a pen that draws Bézier curves. Each area is named, carries
holes that take off what they cover, and reports its area and perimeter in real
units. The work saves beside the drawing and exports to CSV.

## What it does

- **Open and read a PDF.** Pan with the wheel button held, zoom about the
  cursor with the wheel, step through pages. Only the visible part of the page
  is rendered, so a large sheet at working resolution costs nothing extra.
- **Calibrate.** Pick two points, give the real distance between them, and the
  page has a scale. It is a property of the page, so it survives zooming and is
  saved with the work.
- **Trace areas.** Click to place corners, drag while placing to pull a curve
  out of an anchor. Right-click closes the outline. Areas are computed from the
  curve itself rather than from a flattened approximation of it.
- **Edit what is traced.** Move anchors and their handles, add an anchor to an
  edge, take one away, flip a corner into a curve. Undo and redo throughout.
- **Punch holes.** A hole takes off only the area it actually covers, and can
  be edited and removed like any other outline.
- **Aim precisely.** Placement snaps to existing anchors, a modifier holds a
  placement square to the last one, and a magnifier follows the cursor.
- **Report.** Each area carries a name, a colour and a visibility toggle, with
  its area and perimeter beside it. Save writes a JSON sidecar next to the
  drawing and reopening the drawing brings the work back. Export writes one row
  per area to CSV.

## Running a packaged copy

Unzip it anywhere and run `Measure.exe`. The PDFium library beside it is loaded
at start-up from that folder, so keep the two together. Nothing is installed
and nothing is written outside the folder you open drawings from.

The executable is not code-signed, so the first run on a machine brings up a
warning from Windows about an unrecognised application.

## Building it

Needs a Rust toolchain. The PDFium dynamic library is a native binary that this
repository neither builds nor carries: put a copy next to the executable, or
install one system-wide, and the application will find it. Builds of it are
published for every platform; the `pdfium-render` crate's documentation points
at them.

```
cargo run                 # a development build, with the library in target\debug\
cargo test                # the geometry, the document, and the key bindings
scripts\package.ps1       # a release build, zipped, ready to hand over
```

`scripts\package.ps1` expects `runtime\pdfium.dll` and the `LICENSE` file that
came with it, and refuses to package without both — the licence has to travel
with the library.

`scripts\notices.ps1` regenerates `THIRD-PARTY-NOTICES.md` from the dependency
graph. Run it whenever a dependency changes.

## What it is not

It measures areas on drawings, and does that one thing thoroughly. It has no
linear or count tools, no layers or markup, no volumes or elevations, no batch
processing, no plugins, and it talks to no network. `CLAUDE.md` records what is
in scope, what is deliberately out of it, and why.
