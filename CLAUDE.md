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

Phase one is delivered and tagged `v0.1.0`: areas measured on a single
calibrated drawing. Phase two builds a site take-off on that foundation. The
list below is the whole of phase two; §6 says what order it is built in.

**In scope**

- Everything phase one delivered: open a PDF, pan and zoom, calibrate, trace
  and edit Bézier areas, holes, names and colours, area and perimeter in real
  units, CSV export.
- **A project holds several drawings**, saved as a project file that names
  them, rather than a sidecar beside one of them.
- **One site space.** Every outline is stored in real-world coordinates. A
  drawing is placed into that space and is a window onto it, not the place the
  geometry lives.
- **Registering a sheet** against an already-placed one, by picking two points
  common to both. The scale is inherited, and the mismatch left over is
  reported rather than absorbed.
- **Groups**: an area type, carrying an ordered build-up of layers with
  thicknesses, tagged onto areas. Totals by group; areas named from the type
  and their ordinal.
- **Quantities**: the group's total area against each layer's thickness, the
  lines a bill is made of, exported.
- **Deduction by containment**, derived and reported gross, deducted and net.
- **Checks**: a site boundary against the sum of net areas, and overlap within
  a group. Both report; neither resolves.

**Explicitly out of scope.** Do not build, scaffold, or leave hooks for any of
these:

- Count tools, layers as a drawing concept, annotations, markup
- Surfaces, cut/fill, or 3D of any kind. Volume here is plan area against a
  layer thickness, and nothing more.
- Pricing, rates, or anything that turns a quantity into money
- Batch or unattended processing
- Any form of plugin system, scripting, or extension point
- Any network, sync, licensing, or telemetry feature
- Any alternative UI backend or FFI boundary

There is no second frontend. There is no server. Do not write abstractions that
anticipate one.

**One exception, decided at slice 8.** The tool strip and the measurement panel
carry disabled buttons for Line, Rectangle, Ellipse, Polygon, Polyline, Type,
Eyedropper and Length, holding the layout a later, separate project is meant
to fill in. They are labels and nothing else: no state, no tool, no code path
reaches them, and no other code accounts for them.

Shortcuts are reserved the same way. `Shift+Alt+C` is held for a format
painter, and `Shift+Alt+L` for Length, and neither may be bound to anything
else. The second column of the interface is deliberately empty for the same
reason: it holds the space for a later project's tool groups. No shortcut may pair Ctrl with
`C`, `X` or `V`: the window layer turns those into clipboard events and returns
before a key event exists, so they cannot reach this application at all.

Every one of them stays out of scope **for this project** and will not be
wired up here. They are kept in the interface deliberately, so the shape of
the palette is settled before anything grows into it. Do not grow this
exception into a hook: reserving a letter is not the same as leaving a seam
to build behind.

---

## 3. Architecture — decided

- **Single binary crate.** Modules: `pdf`, `viewport`, `geom`, `tools`, `doc`,
  `ui`. Do not add a module without asking. Do not split into a workspace.
- **Fixed dependency list:** `eframe`/`egui` (wgpu backend), `pdfium-render`,
  `kurbo`, `i_overlay`, `serde`, `serde_json`, `rfd`, `csv`. **No new
  dependency without asking first**, including dev-dependencies. `i_overlay`
  was added at slice 8 for one job only: the area a hole and its outline have
  in common. Boolean geometry is a well-known source of quiet wrongness and
  this is a measuring tool, so it is not hand-rolled.
  `embed-resource` was added at slice 10, as the one build-dependency: the icon
  on the executable is a Windows resource compiled into the file, and nothing
  in the language puts one there. It builds the icon in; none of it ships.
- **All geometry is stored in site space**, in real-world units, from phase two
  onwards. Never store screen pixels, and never store page points: a drawing is
  a window onto the measure, not the place it lives. Screen position is derived
  in two steps — `page = placement⁻¹(site)`, then
  `screen = page * viewport.zoom + viewport.pan` — so re-registering a sheet
  moves the window and never the work.
- **A drawing's placement is a similarity transform** from its page points into
  site space: uniform scale, rotation, translation. Nothing else. A sheet that
  needs shearing to fit is a sheet that is wrong, and it should be said so
  rather than accommodated.
- **Calibration is real-world units per PDF point**, per sheet, and it is what
  establishes the first placement. A sheet registered against a placed one
  inherits that scale.
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
  order. Static linking was weighed at slice 10 and refused: there is no source
  build to link against, so a static library means building PDFium with its own
  toolchain and keeping that matched to the compiler — a standing burden, to
  save one file in a folder. The library ships beside the executable, which is
  where the loader looks first anyway.
- **Areas are computed analytically** via `kurbo`'s signed area. Do not flatten
  curves to compute area; flattening is for rendering and hit-testing only.
  One exception, decided at slice 8: a hole is only allowed to take off the
  area it actually covers, and clipping it to its outline needs boolean
  geometry, which needs flattened outlines. A hole lying wholly inside its
  outline — the ordinary case — still subtracts its exact analytic area; only
  one that overhangs is measured from the clipped polygon, at a flattening
  tolerance far below anything a drawing carries.
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

There is one notion of **what is in hand**: an area, addressed by one of its
outlines. Every tool reads it and no tool keeps a second idea of what is being
worked on. The anchor tools show and edit only that area; a click on another
takes that one up instead of editing at arm's length; whatever the pen has just
traced is in hand the moment it closes, so a hole goes into it without a
selection step. Nothing is in hand after a page change or an undo, because a
pair of indices means nothing against a document it was not taken from.

**Grouping: decided, not built here.** Areas will need grouping — five concrete
path areas reported individually *and* as one concrete path total — and a group
will carry an ordered build-up: surface preparation, sand, roadbase, slab, each
with a thickness, summing to a level reduction, so one traced outline yields
several lines of quantities. That work belongs to a later, separate project.
When it comes, a group is a **tag, not a container**: a group is its own entity
with its own identity, and a measurement carries the id of the one it belongs
to. Measurements stay a flat list, so every index, hit-test and undo snapshot
keeps working, and renaming a group is one edit in one place rather than five.
Nothing here anticipates it — no field, no hook, no seam. The sidecar gains the
field with `#[serde(default)]` when it exists, and reads today's files as
ungrouped work.

Naming follows from that. A type is named once — "concrete footpath" — and each
new area of it is the next one: concrete footpath 1, 2, 3. Four points for
whoever builds it:

- **Store the ordinal, derive the name shown.** A member holds its number and
  no name of its own; what is displayed is the group's name and that number, so
  renaming the type renames every member at once. Materialising the string
  instead leaves five members called "concrete footpath 3" after the type is
  renamed, and nothing says so. A typed name overrides both, for when a place
  reads better than a number. Today's `name: String` becomes `Option<String>`,
  and every sidecar written before groups reads back as a typed name — which is
  right, since those names were typed.
- **Groups live at project level, not per page.** A site spans sheets, so the
  numbering carries across them and the total sums across them. Filing groups
  under a page is the mistake to avoid.
- **The next ordinal comes from a counter on the group**, not from counting its
  members, or deleting one makes the next area reuse a number.
- **Ordinals never renumber.** A deleted member leaves a gap; numbers keep
  pointing at what they pointed at, which is what a checked report needs.

Today's naming has the same flaw in miniature: a new outline is called
`Area {count + 1}`, so deleting one and tracing another repeats a number. It is
left alone deliberately — the fix is the group's counter, and the group does
not exist here.

**How areas relate to one another: intent, not built here.** Areas partition
the site. They do not overlap, they leave no gaps, and their areas sum to the
site's. A build-up is vertical — one area carries several material layers over
its own footprint — so two areas overlapping would claim both build-ups over
the ground they share, which is why a partial overlap is always a defect and
never a feature. Snapping is the guard against the opposite fault, a gap
between areas meant to abut. Note that this needs no grouping to judge: areas
never legitimately overlap, whatever they carry.

Containment is the one exception, and it is a deduction rather than an overlap:
an area lying wholly inside another takes its area off that one. A gross 1200
measured to be sure the whole of it is treated somehow, a 100 measured within
it, and the enclosing area nets 1100 while the site still sums to 1200. When
that is built, the deduction is **derived, not stored**: each area reports
gross, deducted and net, so that moving or deleting the inner area corrects the
outer one by itself, and both numbers stay on the page to be checked. Holes
stay what they are — a void measured as nothing, not a deduction for something
measured elsewhere. Once there is a site boundary, the sum of the net areas
against it catches both faults in one number: short means gaps, over means
overlap.

None of this is built here, and none of it may be resolved silently: an area
lying inside another does not reduce it today, and the deduction is made by
hand, by punching a hole where the inner area sits.

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

### Phase two

1. A project holds more than one drawing: add a second, see both listed, switch
   between them, save a project file that names them and reopen it. A phase-one
   sidecar comes in as a one-drawing project.
2. Geometry moves into site space. Calibrating a sheet places it; outlines are
   stored in real-world coordinates and the drawing becomes a window onto them.
   Nothing on screen changes — that is the check — but the file holds one
   measure rather than one per page.
3. Register a sheet against a placed one by picking two common points. It
   lands, the scale is inherited, and the residual is reported. Areas traced on
   either sheet are one measure, and the export is one list.
4. Areas show through: one traced on a neighbouring sheet appears where it
   falls on this one, so a matchline reads as continuous.
5. Groups exist: name a type, assign areas to it, see the list two-level with a
   total per group. Areas take their name from the type and their ordinal.
6. A group carries an ordered build-up: layers with thicknesses, and the level
   reduction they sum to.
7. Quantities: the group's total against each layer, one line apiece.
8. Deduction by containment: gross, deducted and net, derived rather than
   stored.
9. Export the quantities alongside the areas.
10. The checks: a site boundary against the sum of net areas, and overlap
    within a group. Both report; neither resolves.

### Phase one, delivered and tagged `v0.1.0`

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
9. Save and reload a project sidecar; export CSV. Perimeter is reported
   alongside area, in the list and in the export — §2 promises it and no
   earlier slice delivered it.
10. Packaging and buffer.

Do not start a slice before the previous one has been run and accepted.

**Current slice: phase two, 1 — not started.** Phase one is accepted whole and
tagged `v0.1.0`. Slice 10 closed it out:
`scripts\package.ps1` builds a folder that runs on a machine with nothing
installed — the executable, the library it loads, and the notices both carry —
and zips it. The mark on the window and on the executable is drawn at build
time from four coordinates rather than decoded from a file. The notices are
generated from the dependency graph by `scripts\notices.ps1`, so keeping them
accurate is one command. A released build is a window and nothing else.

The next piece of work starts by agreeing what it is and what finishing it
looks like, the same way each slice did. §4 records three decisions made for
work that belongs elsewhere — grouping, how areas relate to one another, and
how a type's areas are named — none of which is built here.
Update this line when a slice is accepted.
