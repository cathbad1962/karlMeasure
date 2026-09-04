# Decisions

A record of what was discussed and what was settled, in date order, newest
last. It is history: it says what was decided and when, not what to build next.
Nothing here is a schedule, and no entry authorises work that has not been
asked for in the session doing it.

---

## 2026-09-05 — Forward planning abandoned

The multi-drawing work was run for the first time, against synthetic drawings
generated for the purpose. It did what had been asked of it and the thing it
had been asked of was wrong: a second drawing is added and then calibrated
independently, so two sheets become two unrelated coordinate islands rather
than one continuous measured space. In the code this is
`calibrations: HashMap<Sheet, Calibration>` — one calibration per sheet,
each derived from its own picked points, none of them related to any other.

Two things were decided about how the project is run.

**The plan is gone.** CLAUDE.md carried a numbered plan through the whole of
phase two and a line naming the step in hand. Both were deleted in `f011c73`.
The plan had been written before the multi-drawing behaviour had ever been run,
and it decomposed "a project holds several drawings" separately from, and three
steps ahead of, "how a drawing joins the project" — so the first was built
without the second, which is exactly how independent calibration got in. The
conflict had even been noticed and written down as a note deferring the fix
rather than treated as a blocker. A defect known in advance blocks the work; it
is not something to record and schedule.

**CLAUDE.md is history.** It is a record of what has been built and decided,
not a specification to work to. It is not to be cited to justify a direction.
Work proceeds on instructions given in the session doing the work.

---

## 2026-09-05 — Bringing in a drawing by matching reference points

Settled in conversation. What was built from it is the entry below.

### What a reference point is

- A tool of its own places them.
- **They belong to the site, not to a drawing.** Any sheet covering that ground
  shows them, whether or not anyone identified them on it.
- They go on identifiable features, as far apart as possible.
- Any number of them, added at any time, including onto a sheet that was itself
  placed by matching rather than by calibration — a drawing brought in later
  may need points the originally calibrated drawing never carried.
- They can only be placed on a drawing that has been calibrated or placed, not
  before.

### What "matchline" means here

The reference points on each drawing matching each other. It is **not** the
printed matchline at the edge of a sheet. The edge of the page is irrelevant,
and adjoining sheets overlap by however much ground they have in common.

### Bringing in a drawing

- The first drawing is calibrated. Every drawing after it is placed by
  matching, and never by calibration.
- A new drawing cannot be brought in until the placed drawing carries at least
  two reference points. Short of that it is **refused**: the drawing does not
  load, and the person is told to place the points first. It is not a warning
  that can be clicked past.
- The new drawing loads into the main view, with a dropdown of the available
  reference points alongside it. A point is chosen from the dropdown, then
  clicked where it falls on the new drawing. Repeated for a second point.
- **Scale is derived from the match, not inherited.** Sheets are at varying
  scales and it does not matter: two points whose real-world separation is
  already known give the incoming sheet both its scale and its rotation.
- Sheets can arrive rotated. That is normal and expected.
- **The app does not check the match.** Because scale comes out of the two
  points, they fit exactly by construction and a misidentified point would
  place and scale the whole sheet wrongly without looking like an error. This
  was put and accepted: the person doing the work is responsible for getting
  it right.
- **A registration is not redone.** It has to be correct when it is made. The
  remedy for one that is wrong is to remove the drawing from the project and
  bring it in again, losing whatever was traced on it.

### Files

One page per file. A PDF with more than one page is **refused outright**, with
a message asking for single-page files. Pages are registered one at a time.

### What the application did at the time

Recorded so it was not mistaken for agreement. The entry below says which of
these changed:

- Any drawing can be calibrated independently, at any time, including the
  second and every one after it.
- A drawing is a file *with* pages, moved through inside one drawing. Multi-page
  files are accepted and paged through.
- There is no reference point tool, no registration, and no site space:
  geometry is stored in page points, per sheet.

---

## 2026-09-05 — Reference points and matching, built

Commit `0221838`. The entry above is what was asked for; this is what the
application now does, and what it still does not.

### Built as decided

- **The `x` tool** marks a place on the site, drawn as a target: two concentric
  circles, the quadrants alternating light and dark, a crosshair through them,
  at a fixed size on screen so it stays findable at any zoom. The magnifier
  comes up while placing one.
- **A reference point belongs to the site.** It is held once for the whole
  project, in site millimetres, under no sheet at all. Every placed sheet draws
  the ones falling on the ground it covers, whether or not anyone identified
  them there.
- **Labels** default to a number so that none is nameless, and are typed over
  with what the place is actually called. The number comes from a counter that
  only ever climbs, so removing one never lets its number come round again.
- **A sheet is placed by a similarity transform** into the site — scale,
  rotation, translation, and nothing else. Calibrating the first drawing is
  what places it, and is the one placement that derives from nothing else: it
  follows from the calibration rather than being stored beside it, so the two
  cannot fall out of step.
- **Matching a drawing in.** Two places are chosen in turn from a picker and
  clicked where they fall on the incoming sheet. Nothing is put on the drawing
  by choosing: the choice says which place is about to be pointed at, the click
  says where it is. The two pairs fix the transform exactly.
- **Scale and rotation come out of the match**, never inherited. A sheet at
  another scale, laid out at another rotation, lands where it belongs. The
  derived scale is reported, because it is the one thing about the match that
  nobody stated.
- **Bringing in a drawing is refused** until two places are marked, since those
  are what it would be matched against. The menu item is disabled and says so.
- **Calibration is offered only while nothing is placed.** This is what stops a
  second drawing being given a scale of its own. It also means the first
  drawing can no longer be recalibrated.
- **No re-registration.** A drawing placed wrongly is removed and brought in
  again, so removing a drawing is possible, and giving up part-way through a
  match does exactly that.

### Decided while building

Not covered by the conversation, and open to being changed:

- **The reference points list sits in the right-hand panel, above the
  measurements.** That panel was previously for measurements alone. The points
  had to be nameable somewhere, and the second column is reserved.
- **The site is placed by the first calibrated sheet in sheet order.** With
  calibration now offered only once, there is only ever one to choose from.
- **Two clicks on the same spot are refused** rather than yielding a placement
  with no scale. The point goes back in the picker to be identified again.
- **A measurement reads off however its sheet came to be placed.** Area and
  perimeter come from the sheet's scale and the unit the site reads in, so a
  matched sheet is traced on like any other. Without this a matched sheet would
  be placed and still unmeasurable, and the match would deliver nothing.

### Not built

- **Multi-page files are still accepted.** A PDF of several pages comes in as
  one drawing and only its first page is matched; the rest stay unplaced and
  unusable. The decision was to refuse such files outright and take pages one
  at a time.
- **Geometry is still stored in page points, per sheet**, not in site space.
  Areas and perimeters are right, because the sheet's own scale is applied to
  them, but an outline itself is not held as a place on the site. Only
  reference points are.
