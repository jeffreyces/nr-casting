# v0.3.0 - Nightreign Casting Plus, and loadout profiles

The mod now has a name you can say out loud, and assignments can be saved as named profiles and
switched mid-game.

**Upgrade is the DLL.** Your existing assignments become the `default` profile on first load; nothing
is lost, and nothing needs editing.

## New

- **Loadout profiles.** A profile is a named assignment sheet: which spell each of the eight inputs
  casts, plus that profile's own `balanced` and `skip assigned in wheel` switches. Save the setup you
  use for a boss, one for exploring, one for a spellblade - the staff in your hand stays the same, only
  the assignments change. Profiles never touch your equipment, memory slots, inventory or save.
- **Two ways to make one.** `create new` at the bottom of the profile list starts an **empty** sheet to
  build a loadout in; `save as` copies the live one, for forking a setup you already tuned.
- **Switch without closing the panel.** `Ctrl+1` to `Ctrl+9`, one digit per profile, autobound in the
  order you create them, and working with the panel open as well as in the world. `Ctrl+1` belongs to
  `default` from the first launch, so the digit a new profile gets is the next one. The modifier is `profile_modifier` in the ini (`Ctrl`, `Alt`, `Shift` or
  `Super`), and a profile's digit can be changed in the panel - a digit another profile holds is
  refused rather than stolen.
- **The live profile is always visible.** Top right, on the title band, in the faded grey the panel
  uses for state: `Profile: boss (Ctrl+2)`.
- **Each profile shows the mode it will switch to.** The list reads `default (Ctrl+1, as-is)` or
  `boss (Ctrl+2, attuned, skip)` - the profile's own `balanced` and wheel-skip values, which switching
  publishes. Without that label a switch could turn balanced mode on and make a free-mode assignment
  read as unbound, with nothing on screen to explain it.
- **A profile row of its own**, under the `balanced` toggle: the profile dropdown, its hotkey, and
  `save as` / `rename` / `delete`. A rule under that row separates the two gameplay rows from the
  panel's own controls. `default` cannot be renamed or deleted, and deleting the profile you are on
  lands you back on `default`.
- **Profiles are the same list for every character.** Each character keeps its own file, because the
  spells it can cast differ.

## Changed

- The panel's title and the mod's display name are **Nightreign Casting Plus**. The DLL, the archive,
  the repository and the me3 profile keep their `nr_casting` names, so existing installs and updates
  are unaffected.
- The slot file gained `[profile <name>]` sections and a format marker. A file without them is read as
  `default` and rewritten once, in place.

## Verified

142 unit tests green (17 for the profile model: parsing, round-trip, autobind, refusals, renaming,
deleting, the hotkey guards). `cargo xwin check --target x86_64-pc-windows-msvc` clean.

## Known limitation

An **older build** of the mod reading a profile file ignores the section headers and sees the union of
every sheet as one set of assignments. It cannot switch profiles anyway, so this only matters if you
downgrade.
