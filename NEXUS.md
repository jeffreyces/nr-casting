[center][size=6][b]Two Spells Per Catalyst[/b][/size]
[size=4]Four spells on two staves — with Elden Ring doing the casting[/size][/center]

[line]

[b]Two spells per staff.[/b] R1 and R2 cast the right-hand catalyst's two spells, L1 and L2 the
left-hand one. That is four castable spells across two staves, swapped instantly, without opening
the menu or touching the spell wheel mid-fight.

This is not a custom casting implementation. The mod rewrites the R2/L2 reports into shoulder
presses before the game reads the pad, so Elden Ring runs its own cast: real FP costs, real
requirements, real animations, real i-frames. If the game would not let you cast it, the mod will
not either.

[line]

[size=5][b]Features[/b][/size]

[list]
[*][b]Four spells, two catalysts[/b] — a bumper spell and a trigger spell per hand.
[*][b]In-game assignment[/b] — an overlay panel lists your equipped catalysts and lets you pick
either slot from the spells your character actually owns and can cast.
[*][b]No menu detours[/b] — bind without the panel: hold the bind key and press the input you want
to assign the spell currently selected in the wheel.
[*][b]Ash of war still works[/b] — hold R3 and press L2 and the trigger passes through untouched.
[*][b]Nothing edited[/b] — no regulation changes, no save edits, no data files. Your assignments
live in a small text file next to the DLL.
[*][b]Offline only[/b] — built for me3 with online play disabled, so EAC never loads.
[/list]

[line]

[size=5][b]Controls[/b][/size]

[list]
[*][b]F8[/b] — open/close the assignment panel
[*][b]R1 / R2[/b] — cast slot 1 / slot 2 of the right-hand catalyst
[*][b]L1 / L2[/b] — cast slot 1 / slot 2 of the left-hand catalyst
[*][b]R3 + L2[/b] — vanilla L2, for the weapon's ash of war
[*][b]Hold L3 + R1/R2/L1/L2[/b] — bind the spell currently selected in the wheel to that slot
[/list]

The panel hotkey and the bind key are both configurable in [code]nr_casting.ini[/code].

[line]

[size=5][b]Requirements[/b][/size]

[list]
[*]ELDEN RING, offline. Online play and EAC are not supported and not tested.
[*][url=https://github.com/garyttierney/me3]me3[/url] (Mod Engine 3) to load the DLL.
[*]A controller — the mod works at the XInput layer.
[/list]

[line]

[size=5][b]Installation[/b][/size]

[list=1]
[*]Extract the archive anywhere outside the game folder.
[*]Copy [code]nr-casting.me3[/code] into your me3 profiles folder:
[code]Windows: %APPDATA%\me3\profiles\[/code]
[code]Linux:   ~/.config/me3/profiles/[/code]
[*]Edit the [code]path[/code] line in it to point at [code]nr_casting.dll[/code].
[*]Launch with [code]me3 launch -g eldenring -p nr-casting[/code].
[/list]

The download contains four files and nothing else. Everything the mod needs it generates itself,
in an [code]nr_casting\[/code] subfolder beside the DLL, so it never clutters a shared mods folder.
Deleting that subfolder resets the mod.

[line]

[size=5][b]Configuration[/b][/size]

[code][ui]
hotkey = F8

[input]
bind_modifier = L3

[display]
hud_mirror = off

[logging]
debug_log = off[/code]

[code]hud_mirror[/code] shows the right-hand slot 1 spell on the game's HUD, and locks the D-pad
wheel while enabled.

[line]

[size=5][b]How it works[/b][/size]

Elden Ring already casts on R1 and L1 with a catalyst in that hand — it casts whichever spell is
[i]selected[/i]. The mod rewrites the R2 and L2 reports into shoulder presses, then points the
selected magic slot at the spell bound to that input for the duration of the press, restoring it
the moment the input is released. Two presses on the same catalyst therefore reach two different
spells, and a full spell wheel is never displaced.

[line]

[size=5][b]Compatibility[/b][/size]

[list]
[*][b]me3[/b] — supported.
[*][b]Mod Engine 2[/b] — discontinued upstream, but the DLL is a plain native plugin.
[*][b]Other overlay mods[/b] — both hook the DX12 swap chain and window procedure; expect fights
over input.
[*][b]Spell or selection mods[/b] — direct conflict, both write the game's magic selection.
[*][b]Regulation mods[/b] — no conflict; this mod ships no param edits.
[/list]

[line]

[size=5][b]Notes[/b][/size]

[list]
[*]Spell and weapon names are compiled into the DLL, so the mod works without unpacking anything.
[*]After a game update the mod falls back to the nearest known offset table and validates it before
touching memory; if validation fails it stays out of the way and says so in its log.
[*]If something misbehaves, the log in [code]nr_casting\nr_casting_probe.log[/code] records what the
mod did, in order.
[/list]

[line]

[size=4][b]Credits[/b][/size]

Built on [url=https://github.com/vswarte/fromsoftware-rs]fromsoftware-rs[/url] (structure and RVA
definitions) and [url=https://github.com/veeenu/hudhook]hudhook[/url] (DX12 ImGui overlay).
Everything it reads about spells comes from your own copy of ELDEN RING.
