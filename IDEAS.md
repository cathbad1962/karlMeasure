# Ideas

Somewhere for what is worth doing but is not being done. Nothing here is a
commitment, and nothing here is a scope: something is taken from this list
deliberately and agreed before it is built.

One line each, with why it might matter. If an idea turns out to be a decision
about how something already built should work, it belongs in CLAUDE.md instead,
not here.

## Raised and not scheduled

- **Picking a drawing from a list rather than cycling.** The strip along the
  bottom steps one at a time, which is fine for a handful and tiresome for
  forty.

- **Vector extraction from the PDF.** PDFium can hand back path segments, so an
  outline could be traced from the drawing's own geometry rather than by hand.
  Deliberately unbuilt; the hand-traced path is the one that gets checked.
- **Tracing across a matchline in one outline.** Decided against for now: parts
  are traced per sheet and the group adds them up. Would need a half-traced
  outline to survive a sheet change.
- **All placed sheets in one canvas.** Pan and zoom across the whole site
  rather than a sheet at a time. Several slices of texture management before it
  is usable, and the site space has to exist first.
- **Code signing.** The package is unsigned, so every new machine warns once.
  Needs a certificate and a decision about who signs.
- **A test that opens a PDF.** The arithmetic is covered end to end, but the
  link from a real page to page-space coordinates rests on acceptance runs. A
  synthetic PDF generated at test time with a known rectangle on it would close
  the last gap in the numbers.
- **Gaps between areas meant to abut.** The sibling of the overlap check, and
  harder: a gap has no boundary unless something says what should have abutted.
  Snapping is the guard that prevents it in the first place.
