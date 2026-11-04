# v0.2.23 — ERD Tools works alongside the panel, and the log stops carrying paths

One compatibility fix, one privacy fix, one diagnostics fix.

**Upgrade is the DLL.**

## Fixed

- **ERD Tools no longer crashes when the panel opens.** With both loaded, opening the panel killed
  the game a second or two later, inside ERD's own code. Reproduced locally and bisected by
  capability rather than by guesswork — window only, plus a D3D11 device, plus a swap chain that is
  never presented: all fine; a single `Present`: crash, at a different ERD offset each time. Turning
  every hook in this mod off did **not** stop it, so the input detours were never involved. ERD hooks
  the present path process-wide, and a second swap chain presenting in the same process leaves its
  per-frame code reading stale state.
  The panel therefore no longer owns a swap chain at all: it renders into a texture, copies that to a
  DIB and blits it into its own window, so it calls no `Present`. Cost is one GPU-to-CPU copy per
  drawn frame — reported as `present ok (copy under 1 ms)` at 1623x720 — and nothing at all while the
  panel is closed. Opening costs about 130 ms the first time (device, then surface, then the font
  atlas) and 4-5 ms afterwards; closing is 0 ms, with the game's next frame arriving in the same
  millisecond.
- **The log no longer carries your paths.** `nr_casting_probe.log` was written unredacted; the
  existing redactor only ran on the diagnostics report. Redaction now happens at the log writer, so no
  call site can reintroduce it: a path inside a mod manager's folder keeps only its tail from `me3`
  onward (`me3\config\profiles\<profile>\<mod>\nr_casting`), and every other path becomes `<path>`.
  The account segment, the manager's own folder and the drive letter all go.
- **Crash lines name the module that actually faulted.** A report read `in Elden_Ring_g` while the
  base address belonged to `erdGameTools.dll`, because the header kept the end of a 96-character read
  of the module path and `GetModuleFileNameW` truncates from the front. It now uses the same
  `MAX_PATH` lookup the frame list below it already used, so the line reads
  `in erdGameTools.dll (base …, offset 0xa64f9)`.

## Changed

- **The log's first line names the build**: `=== nr_casting 0.2.23 (built …)`. Several builds share a
  version during a test cycle, and "which build was that?" was previously unanswerable from the log.
- **Open, close and present costs are logged**: `panel: open in 133 ms`, `panel: close in 0 ms`, and
  the per-second panel line reports the copy cost. A lag report is now a number rather than a
  description.
- Build artefacts that had once been committed under the crate's `target` directory are no longer in
  the repository.

## Verified

118 unit tests green (2 for the row copy between two pitches, 2 for the redaction rule). `cargo xwin
check --target x86_64-pc-windows-msvc` clean. ERD Tools was loaded alongside in a local session with
the panel opened repeatedly and the game alive throughout; ERD's own log shows its overlay
initialising and its menu installed, and no crash line was written. The blit path is **not** exercised
on native Windows in this release — it was tested under Proton only.
