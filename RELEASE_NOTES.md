# v0.2.10 — the hook that never was, and a quieter binary

Two fixes that turned out to be one story. The pad hook has been patching the *export's prologue*
on every machine this mod has ever run on, because the code that should have routed the game's own
import slot never worked — and a binary that patches another module's code reads, to a scanner, like
a loader. That is now the pointer store it was always meant to be, and a few things that made the
DLL look worse than it is are gone.

**Upgrade is the DLL.** Nothing else changes; your ini and assignments are untouched.

## Fixed

- **The pad hook finally takes the clean route.** `XInputGetState: routed the game's import slot to
  our detour … (no code patched)`. Until now the import walk read the PE header by hand, got a
  nonsense `e_lfanew` on both Windows and Proton, and fell back to rewriting the prologue of a
  function the game calls fifty to a hundred and twenty times a second. It now asks the loader
  (`RtlImageNtHeader` / `RtlImageDirectoryEntryToData`) and writes one aligned pointer instead. The
  log prints what the game actually imports — `import walk: XINPUT1_4.dll: [ordinal 3, ordinal 2]` —
  so a future "the hook is not being called" report starts from a fact rather than a guess.
- **The report's input line said two things and meant one.** `N with no controller, 0 unplugged` was
  a single counter plus a hardcoded zero, so a wireless pad that dropped nine times in a session
  looked like a probe that had never seen a controller. It is now one number: reads the driver
  refused.
- **"Open the file manager" no longer launches `explorer.exe` by name.** The shell opens the folder
  directly. The file is not pre-selected any more; the folder is.

## Added

- **The pad is described in the report.** `pad: wireless, gamepad, flags 0x0004, battery
  alkaline/low` — transport, device class and battery state, read once from XInput. A stutter that
  only appears with a controller attached is a completely different problem depending on those three
  answers, and no earlier report could tell them apart.
- **`[input] pad_hook = on|off`.** `off` installs no XInput hook at all: pad casting stops, mouse
  casting is untouched. It exists so "is it our code, or merely our presence in the call path" can be
  answered with one ini line — the guided run keeps every hook installed on purpose, and unplugging
  the controller removes the game's calls and our hook at the same time.
- **A packaging guard.** `tools/package.sh` now fails if the built DLL has no resource section, and
  prints the version and signature state either way. 0.2.9 shipped without one.

## Removed, for the scanners

None of this changes what the mod does in game.

- **The write probe is gone.** It snapshotted 132 bytes of the magic-slot struct, wrote a sentinel,
  read it back, restored it and compared every byte — once per process, to prove the write path
  before any cast. That sequence is what a loader does, and it is not worth the shape in the binary.
  The same evidence now comes from the first real cast: the write the mod has to make anyway is read
  back, and a failure is logged once. The offsets file says `read verified` instead of
  `read and write verified`.
- **The clipboard is gone.** Copying the report's path away was one button's convenience and cost
  four clipboard APIs in the import table. The note now shows the path; the folder still opens, so
  the file can be dragged onto the issue page as before.
- **`keybd_event` → `SendInput`** for the Recluse hand-off. Same effect, current API.
- **The DLL identifies itself.** Company name, product, description and version are embedded
  (`Properties → Details` shows them), so a quarantined file names its product instead of appearing
  as an anonymous payload. It remains unsigned: that needs a certificate, and it is the next step.

Import table: 183 → 178.

## If you are upgrading from 0.2.9

- If your workflow was "copy the path from the clipboard", copy it from the note text instead.
- The report's input line now reads `… reads refused by the driver …` with one number.
- Nothing in the ini needs editing, and `pad_hook` defaults to `on`.

## Verified

62 unit tests green. Built with the same toolchain the release uses (`cargo xwin` → MSVC on Linux);
the packaged DLL carries its resource section and reports version 0.2.10.0. The hook change was
executed end to end on Linux/Proton against a real character: `routed the game's import slot … (no
code patched)`, 300 s sampled at p50 16.77 ms with **zero** seconds over 32 ms, and all six bundle
phases free of hitches.

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
