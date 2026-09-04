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

Settled in conversation. None of it is built.

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

### What the application does today that this contradicts

Recorded so it is not mistaken for agreement:

- Any drawing can be calibrated independently, at any time, including the
  second and every one after it.
- A drawing is a file *with* pages, moved through inside one drawing. Multi-page
  files are accepted and paged through.
- There is no reference point tool, no registration, and no site space:
  geometry is stored in page points, per sheet.
