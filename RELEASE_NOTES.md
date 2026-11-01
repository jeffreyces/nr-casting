# v0.2.9 — prove it, then measure it

Two things this release does that the last one could not: it *proves* the game memory it writes to
before touching it, and it makes a stutter report something you run rather than something you
configure.

**Upgrade is the DLL.** Nothing else changes.

## Added

- **`bundle diagnostics & report`, the guided run.** One button, six one-minute phases, one report.
  Load a character, stand still, press start: phase 1 is everything on, then the pointer detours,
  the raw-input hooks, the pad hooks and the housekeeping thread each switch off in turn, then all
  of them, and everything is switched back on at the end. The report is written and opened when the
  six minutes are up. You never edit the ini. Casting stops during the pad phase — that is the
  point of that phase, and it is expected.
- **Hitch counts per phase**, not just the worst gap: how many seconds in each phase had a frame
  over 32 ms, and which second held the worst one. A phase that is 2 ms slower on average but never
  hitches is a different finding from one that hitches once a second.
- **A frame clock that works without a controller.** The old clock was the game's pad poll, so a
  keyboard player saw six empty phases. `GetAsyncKeyState` was not the answer either — measured at
  four calls a minute, not sixty a second, because Elden Ring reads the keyboard through raw input.
  The clock is now the game's own `PeekMessageW`, which runs every frame whatever the input device.
  Measured with no controller connected: p50 19.04 / 18.97 / 19.13 / 19.02 / 19.10 / 19.03 ms across
  the six phases, zero hitches, 0.16 ms spread.
- **The game folder's DLLs, named.** The report now lists every file this process loaded from the
  game's own folder, with its real size and build date, and flags the ones containing strong
  wrapper markers (ReShade, Special K, DXVK, Streamline, DLSS-G). It says plainly that an absence is
  not proof, and how to test one: rename it, run the bundle again, compare phase 1.

## Fixed

- **Opening the bundle prompt resized the panel.** The prompt adds about 110 px of help text, which
  crossed the fifteen per cent content-change threshold that re-latched the layout height — that
  latch feeds the auto scale, so a click on a button changed the scale and the window moved under
  the cursor. The latch now follows deliberate layout changes only: first open, a scale you pick,
  reset layout, list everything.
- **The report's marker list could match by chance.** Two NVIDIA DLLs were labelled "ENB, ReShade"
  because the needle was three bytes; then a 54 MB DLSS DLL was labelled DXVK because four bytes of
  base64 happened to contain `dxvk`. Every needle is now six bytes or more, and a test asserts that
  and feeds the exact bytes that produced the wrong label.
- **File sizes in the DLL section were the read cap**, so six files of exactly 8192 KiB including
  `eldenring.exe`. They are real sizes now.
- **`2.7.1.0 (Unsupported game version 2.7.1.0)`** read like a refusal. It was a fallback notice: the
  mod uses the nearest table and validates it. The line now says which table is in use and that it
  is being checked.
- **No file paths at all in a report**, rather than a path with the account name removed. A
  legal-looking `%USERPROFILE%\AppData\Local\<name>\...` still says who you are and how your
  machine is laid out.

## Casting

- **The field this mod writes is now proven, once per process, before steering turns on.** Reading
  the right values cannot tell a wrong offset that happens to point at an `int` from the right one,
  and the selected spell slot is what a cast depends on. So the mod checks that the object's header
  is a real game object (its vftable sits inside the loaded image), that all fourteen entries hold
  plausible spell ids, then writes a different slot index, reads it back, restores it, and compares
  the whole structure apart from those four bytes. Anything unexpected disables steering and says
  why. The write is live for a few hundred nanoseconds, once, before anything can be cast.

## Verified

62 unit tests green. The guided run was executed end to end on Linux/Proton against a real
character: six phases of 60 s, p50 14.75–15.33 ms, 0 hitches in every phase. The write probe
reports `slot 0 -> 1 -> 0, 132 bytes intact`. The report was reviewed line by line, including the
redaction and the catalogue section, which is where the marker and size defects were found.

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
