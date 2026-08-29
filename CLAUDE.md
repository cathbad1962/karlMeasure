# Project constraints

A desktop application for measuring areas on calibrated PDF drawings.

Read this file before proposing or writing anything. The decisions below are
made. Do not re-derive them, do not propose alternatives unless asked.

---

## 1. Repository hygiene — non-negotiable

- **No organisation, client, or vendor names** anywhere in the repository:
  source, comments, commit messages, README, docs, tests, fixtures, issue text.
- **Describe behaviour, not lineage.** Write "cubic Bézier path with mirrored
  handles; modifier key breaks the mirror". Never describe a feature by naming
  the application it resembles, and never frame this project as a replacement
  for, or comparison against, a named product.
- **Never commit PDFs.** `*.pdf` and `samples/` are gitignored. Real drawings
  carry title blocks identifying projects and organisations.
- **Test fixtures are synthetic** — generated programmatically at test time, so
  the expected area is known exactly and no real drawing is required.
- **No screenshots of real drawings** in the README or docs.

Exception: `THIRD-PARTY-NOTICES.md` carries the PDFium (BSD-3) copyright notice
and crate licences. That is a licence obligation, not a vendor reference. It
does not exist yet — create it when PDFium is first linked, then keep it
accurate.

---

## 2. Scope

**In scope**

- Open a PDF, render a page, pan and zoom.
- Calibrate the page to a real-world scale by picking two points and entering a
  known distance.
- Trace closed areas with a Bézier pen tool, and edit those paths afterwards.
- Named measurements with holes, reported area and perimeter in real units.
- Save to a JSON sidecar. Export measurements to CSV.

**Explicitly out of scope.** Do not build, scaffold, or leave hooks for any of
these:

- Linear or perimeter measurement as a separate tool
- Count tools, layers, annotations, markup
- Volumes, surfaces, elevations, cut/fill, 3D of any kind
- Multi-page batch processing
- Any form of plugin system, scripting, or extension point
- Any network, sync, licensing, or telemetry feature
- Any alternative UI backend or FFI boundary

There is no second frontend. There is no server. Do not write abstractions that
anticipate one.

**One exception, decided at slice 8.** The measurement tool panel carries
disabled placeholder buttons for Length and Polyline, so the panel's intended
shape is visible while it is being built. They are labels and nothing else:
no state, no tool, no code path reaches them, and no other code accounts for
them. The tools themselves stay out of scope until they are given slices of
their own. Do not grow this exception into a hook.

---

## 3. Architecture — decided

- **Single binary crate.** Modules: `pdf`, `viewport`, `geom`, `tools`, `doc`,
  `ui`. Do not add a module without asking. Do not split into a workspace.
- **Fixed dependency list:** `eframe`/`egui` (wgpu backend), `pdfium-render`,
  `kurbo`, `serde`, `serde_json`, `rfd`, `csv`. **No new dependency without
  asking first**, including dev-dependencies.
- **All geometry is stored in PDF page space (points).** Never store screen
  pixels. Screen position is always derived:
  `screen = page * viewport.zoom + viewport.pan`.
- **Calibration is real-world units per PDF point**, stored per page.
- **`kurbo` types are the internal vocabulary** (`Point`, `Vec2`, `BezPath`).
  Convert to `egui` types only at the paint call.
- **Render the visible viewport only**, never the full page — a full-size sheet
  at working DPI exceeds practical texture limits. Re-render on a debounced
  pan/zoom settle; scale the stale texture in the interim.
- **PDFium is a native library, loaded dynamically.** `pdfium-render` does not
  vendor it. The binding looks for the platform library next to the executable
  first, then falls back to a system-wide install. The crate is used with
  `default-features = false` so it pulls in no image-decoding dependency;
  bitmaps are normalised to RGBA by the binding, which needs no image feature.
  Do not swap channels by hand — the binding already reverses its own byte
  order. Revisit static linking at slice 10 if a single-file binary is wanted.
- **Areas are computed analytically** via `kurbo`'s signed area. Do not flatten
  curves to compute area; flattening is for rendering and hit-testing only.
- **Holes are subpaths with opposite winding.** Signed areas sum. No
  special-casing.

---

## 4. Data model — fixed

```rust
enum AnchorKind { Corner, Smooth, Asymmetric }

struct Anchor {
    pos: Point,        // page space
    in_handle: Vec2,   // relative to pos
    out_handle: Vec2,  // relative to pos
    kind: AnchorKind,
}

struct SubPath {
    anchors: Vec<Anchor>,
    closed: bool,
}

struct Measurement {
    name: String,
    outer: SubPath,
    holes: Vec<SubPath>,
    colour: Color32,
    visible: bool,
}
```

Undo is snapshot-based: clone the document vector on each committed operation.
The document is small. Do not build a command pattern.

Hit radius and handle sizes are specified in **logical points**, converted to
page units by dividing by zoom, so precision scales with zoom and behaviour is
identical across display scaling factors.

---

## 5. Working agreement

- **One vertical slice per session.** Each session ends with `cargo run`
  producing something visible and clickable. "The module compiles" is not done.
- **State the acceptance check before writing code** — what I will be able to
  do in the running application when this is finished — and wait for
  confirmation before implementing.
- **No traits until there are two implementers.** No generics where a concrete
  type works. No builders. No newtype wrappers that only forward.
- **Do not refactor unrequested.** Do not "improve" adjacent code while working
  on a slice.
- **Do not build ahead.** If a slice needs something from a later slice, stop
  and ask rather than implementing both.
- **Commit at the end of each working increment**, so a hard reset is cheap.
- Commit messages describe the change only. No project, client, or product names.

---

## 6. Slices, in order

Each line is the definition of done for that session.

1. A PDF opens and a page is visible in a window.
2. The page pans and zooms smoothly; page navigation works. The scroll wheel
   zooms about the cursor; holding the wheel button down and dragging pans.
3. Pick two points, enter a known distance and unit, see a scale readout that
   survives zooming.
4. Click three times to place corner anchors, close the path, see the enclosed
   area in real units.
5. Click-and-drag while placing produces smooth anchors with mirrored handles;
   a modifier breaks the mirror.
6. Select and drag an existing anchor or handle; insert an anchor on a segment;
   delete one; toggle corner/smooth. Undo and redo work.
7. Measurements are named and listed; holes subtract; colours and visibility
   toggle.
8. Snapping to existing anchors; modifier-constrained orthogonal placement;
   magnifier under the cursor.
9. Save and reload a project sidecar; export CSV.
10. Packaging and buffer.

Do not start a slice before the previous one has been run and accepted.

**Current slice: 8 — not started.** Slice 7 is accepted: measurements are
listed, named, coloured and hidden, and holes subtract by winding.
Update this line when a slice is accepted.
