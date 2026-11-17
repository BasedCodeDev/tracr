Vendored vt100 0.15.2 (MIT), from https://github.com/doy/vt100-rust.

Local patch: grid::scroll_up retains rows when the scroll region starts at row
zero, including partial-height regions used for CLI history above a composer.
Interior regions and the alternate screen (zero scrollback capacity) are unchanged.
Regression coverage lives in src/inline_terminal.rs in the parent project.
Removed the now-unused scroll_region_active helper and replaced deprecated
u8::max_value() with u8::MAX to keep local dependency builds warning-free.
Also bound visible_rows after joining history and live rows. Upstream subtracts
the history offset from viewport height, underflowing when scrolling beyond one
page; the iterator must always yield at most one viewport of rows.
