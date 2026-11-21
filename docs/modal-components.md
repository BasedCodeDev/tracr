# Building a modal

Use `crate::modal` for the frame and controls. Keep backend calls, validation,
permission checks, and tasks in the workflow module. Do not copy a dialog's
centering arithmetic or embed buttons in its scrolling paragraph.

`modal::shell` draws a centered frame and returns its padded interior. Preferred
sizes describe the outer frame; padding shrinks on small terminals. `regions`
reserves a scrollable body, two-line status area, action row, and keyboard hints.
Use a local layout inside the body for summaries, fields, trees, or tables.

Scrollable regions use `crate::scrollbar`: a one-column `│` track in dark gray
with a solid half-cell cyan `▐` thumb, shown only on overflow. `modal::body` handles this for
paragraphs; use `modal::static_body` for fixed summaries without scroll controls.
For lists, read the final widget offset after rendering and convert item offsets
to rendered lines (including multiline entries and section headers). Place the
track on the right border alongside the scrolling body, excluding fixed controls.
Unbordered bodies reserve a column only on overflow and must rewrap at that width.
Do not add timers, I/O or total-history queries to calculate scrollbar metrics.

```rust,ignore
let inner = modal::shell(frame, "Example", 80, 24);
let regions = modal::regions(inner);
let max_scroll = modal::body(frame, regions.body, description, scroll);
modal::buttons(frame, regions.actions, &[
    modal::Button { action: Action::Cancel, label: "Cancel", enabled: true, primary: false },
    modal::Button { action: Action::Review, label: "Review", enabled: valid, primary: true },
], focused_button);
```

Store focus, caret, and scroll with the workflow. Clamp scrolling to the returned
limit. `text_field` renders a three-row labeled field and keeps a Unicode caret
visible. `edit` provides character insertion/deletion, arrows, Home/End, and
Ctrl-U; filter control characters from pasted text. Use the changed result to
invalidate validation or update derived fields.

Use `cycle` for Tab/Shift-Tab, passing the enabled controls, and `activate` to map
Enter/Space to a typed button action. Text inputs advance to the next text field
or primary button on Enter; Space remains text. Keep tree/list Enter behavior
local. Render the secondary action before the primary one, follow that order for
Tab, and skip disabled controls. Initialize confirmation focus to the primary
button. Never let queued Enter events accept a newly arrived preview.

The body owns content, not actions: information renderers must not add their own
Copy/Close footer. Put feedback in the fixed status region and short navigation
hints below the actions. Avoid implementation details unless needed for a user's
decision. `buttons` emits a resize message instead of clipping actions that do
not fit. Forms must check their minimum usable dimensions and retain Escape when
showing a resize fallback.

Verify 120×40, 80×24, 60×20, 40×16, and undersized layouts. Test Unicode input,
long paths, keyboard focus, disabled actions, resize preservation, scroll bounds,
and task completion/cancellation. Add workflow acceptance to `scripts/check.py`.
