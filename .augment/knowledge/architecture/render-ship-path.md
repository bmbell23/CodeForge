# The render → ship path, and what it silently drops

Keywords: FrameDiffer, contents_diff, vt100, OSC 8, hyperlink, OSC 52, blit_pane,
wide glyph, wide continuation, artifacts, clickable links.

## The path

1. `render` builds a full frame into `framebuf`, cell by cell, from each pane's
   `vt100::Screen` (`blit_pane`).
2. `FrameDiffer::frame` (`src/main.rs`) feeds that frame into a **second vt100
   parser** that mirrors what the client currently shows, then ships vt100's
   minimal `contents_diff` against the previous screen (or `contents_formatted`
   on a clear/resize/fresh attach).
3. The client writes those bytes to the real terminal.

Step 2 is the important one: **anything vt100 doesn't model is lost between
CodeForge and the user's terminal.** The frame is not passed through — it is
re-derived from vt100's cell grid.

## What that costs

vt100 0.15.2 handles only OSC 0, 1 and 2 (`screen.rs:1670`); every other OSC is
dropped with a debug log. In particular:

- **OSC 8 hyperlinks cannot reach the terminal.** Emitting them from the render
  path does nothing — the mirror swallows them. This is why a pane's own
  hyperlinks (the Claude CLI emits them) are not clickable, and why #105
  (clickable ticket IDs) could not be implemented in the render path. The
  chosen answer there was a client-side WezTerm `hyperlink_rules` entry, which
  matches on the rendered screen and needs nothing from CodeForge:

  ```lua
  local wezterm = require 'wezterm'
  config.hyperlink_rules = wezterm.default_hyperlink_rules()
  table.insert(config.hyperlink_rules, {
    regex = [[\b(SFAP-\d+)\b]],
    format = 'https://ime-ddn.atlassian.net/browse/$1',
  })
  ```

  Making hyperlinks work in-app means tracking them per cell *beside* the
  differ and re-applying them to the diff bytes each frame. That touches the
  ship path on every frame — treat it as its own scoped ticket.

- **OSC 52 (clipboard) is the exception**, and it works by *bypassing* this
  path: `Msg::Output` forwards OSC 52 ranges from a pane straight to the client
  before the bytes ever reach the mirror (`osc52_ranges`). Anything else that
  must survive verbatim needs the same out-of-band treatment — but note it only
  works for sequences that are position-independent. A hyperlink is an attribute
  of printed cells, so it cannot be shipped this way.

## Column accounting in `blit_pane`

A wide glyph (emoji, CJK) takes two terminal columns, but vt100 stores it as a
pair: the glyph, then an **empty wide-continuation cell**. Printing anything for
that continuation makes the terminal advance three columns where the source had
two, so the rest of the row shifts right and its tail overruns the pane's border
onto the neighbour — the stale fragments of #106.

`blit_pane` therefore skips `is_wide_continuation()` cells, and clips a wide
glyph that would land in the rect's final column to a space. `draw_copy_overlay`
and `draw_selection` position each cell with an explicit `MoveTo` so they never
shift a row, but they need the same guard or they paint over half a glyph.

Related hazard: vt100 0.15.2 **panics** (`grid.rs:672`, subtract overflow) when a
wide glyph is written into its own last column. A pane feeding real output can
reach that; see the constitution's "a vt100 panic must not take down the event
loop".
