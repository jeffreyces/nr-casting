# v0.2.8 — the report keeps its log

Two fixes to the diagnostics file, both found by reading a reporter's bundle rather than by testing:
the section that matters most was arriving empty, and the header carried a path that did not need to
travel.

**Upgrade is the DLL.** Nothing else changes.

## Fixed

- **`==== log tail ====` arrived as `(empty)`.** The tail is read with a UTF-8 parse in the report,
  and a log containing a single byte that is not valid UTF-8 makes that parse fail — the whole read
  was discarded and the section printed as empty, in a file whose own header named the log. The log
  carries whatever the game's text does, so this was a matter of time. It is read as bytes and
  decoded lossily now, and a file that genuinely cannot be read says which path failed and why,
  instead of pretending to be empty.
- **The header no longer carries your log path.** `log: C:\Users\<you>\AppData\...` became
  `log: %USERPROFILE%\AppData\Local\garyttierney\me3\...` after redaction — no user name, but
  your machine's own layout in a file that already contains the log it points at. The line is gone;
  `mod`, `exe`, `reason` and `uptime` are what the header says now.

## Also

- The shipped ini's `ui_scale` and `panel_size` comments described the behaviour of two releases ago
  ("the window size no longer changes the font", "auto sizes it from the screen"). They now describe
  what those settings do: `auto` picks the largest scale that fits the window, and the size opens at
  the design size for the screen with its height fitted to the content.

## Verified

52 unit tests green. The report's sections, catalogue (316 spells), sample line and redaction were
checked against a real reporter bundle on Windows 11 / RTX 3080, which is where both defects came
from.
