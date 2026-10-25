[center][size=6][b]Two Spells Per Catalyst[/b][/size]
[size=4]Six spells on two staves — with Elden Ring doing the casting[/size][/center]

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
[*][b]Six spells, two catalysts[/b] — a bumper spell and a trigger spell per hand, plus a third on
the Y chord ([b]Y+R2[/b], [b]Y+L2[/b]); two-handing a staff does nothing, so that input carries a
spell instead.[/*]
[*][b]Assignment without the panel[/b] — select the spell in the game's own menu, then hold the bind
modifier and press the bumper or trigger you want it on. The panel is only a shortcut for this.[/*]
[*][b]In-game assignment[/b] — a panel in its own window lists your equipped catalysts and lets you pick
either slot from the spells your character actually owns and can cast.[/*]
[*][b]No menu detours[/b] — bind without the panel: hold the bind key and press the input you want
to assign the spell currently selected in the wheel.[/*]
[*][b]Ash of war still works[/b] — hold R3 and press L2 and the trigger passes through untouched.[/*]
[*][b]Nothing edited[/b] — no regulation changes, no save edits, no data files. Your assignments
live in a small text file next to the DLL.[/*]
[*][b]Safe to remove[/b] — no save file is ever written and nothing persists in memory, so the mod
can be uninstalled at any time and the game behaves exactly as it did before.[/*]
[*][b]Offline only[/b] — built for me3 with online play disabled, so EAC never loads.[/*]
[/list]

[line]

[size=5][b]Controls[/b][/size]

[list]
[*][b]F8[/b] — open/close the assignment panel[/*]
[*][b]R1 / R2[/b] / [b]Y + R2 [/b]— cast slot 1 / slot 2 / slot 3 of the right-hand catalyst[/*]
[*][b]L1 / L2[/b] / [b]Y + L2[/b] — cast slot 1 / slot 2 / slot 3 of the left-hand catalyst[/*]
[*][b]R3 + L2[/b] — vanilla L2, for the weapon's ash of war[/*]
[*][b]Hold L3 + R1/R2/L1/L2[/b] — bind the spell currently selected in the wheel to that slot[/*]
[/list]

The panel hotkey and the bind key are both configurable in
[code]nr_casting.ini[/code]

[line]

[size=5][b]Requirements[/b][/size]

[list]
[*]ELDEN RING, offline. Online play and EAC are not supported and not tested.[/*]
[*][url=https://github.com/garyttierney/me3]me3[/url] (Mod Engine 3) to load the DLL.[/*]
[*]A controller — the mod works at the XInput layer.[/*]
[*]Keyboard  / Mouse — Also works with Left/Right click (original) But added SHIFT / CTRL + Left / Right to reach full 6.[/*]
[/list]
[line]

[size=5][b]Installation[/b][/size]

[list=1]
[*]Extract the archive anywhere outside the game folder.[/*]
[*]Keep the four files together — the profile already points at the DLL beside it.[/*]
[*]Launch, passing the profile by path:
[code]me3 launch -g eldenring -p "C:\mods\nr-casting\nr-casting.me3"[/code]
[/*]
[/list]

No editing needed: me3 resolves the profile's relative DLL path against the profile's own folder.

Using [b]me3 Manager[/b]? Add the mod through the manager and point it at
[code]nr_casting.dll[/code]

the manager writes its own profile.

Keeping the profile in me3's profile folder instead? Then give
[code]path[/code]
the DLL's full location in [b]single quotes[/b], so backslashes stay literal:
[code]path = 'C:\mods\nr-casting\nr_casting.dll'[/code]
Forward slashes work too, and the path must end at the [i].dll file[/i], not the folder.

The download contains four files and nothing else. Everything the mod needs it generates itself, in an
[code]nr_casting\[/code]
subfolder beside the DLL, so it never clutters a shared mods folder. Deleting that subfolder resets the mod.

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

[code]hud_mirror[/code]
shows the right-hand slot 1 spell on the game's HUD, and locks the D-pad wheel while enabled.

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
[*][b]me3[/b] — supported, including me3 Manager.[/*]
[*][b]Frame-generation and upscaling mods[/b] (FSR/FG) [url=https://www.patreon.com/huutaiii/posts/erss-fg-free-v4-125972766]https://www.patreon.com/huutaiii/posts/erss-fg-free-v4-125972766[/url]:
[code]d3d12.dll[/code]
[code]dxgi.dll[/code]
[code]erss-fg.dll[/code]
 — the panel is a separate window, so it draws regardless of who presents the frame. Casting is unaffected either way.[/*]
[*][b]Class ports with their own HUD[/b] (Recluse and friends) [url=https://www.nexusmods.com/eldenring/mods/11410]Nightreign Recluse Full Port[/url]:
[code]ERNativeUI.dll[/code]
 — no conflict: the panel never touches the game's swap chain, so their on-screen HUD keeps drawing.[/*]
[*][b]Other overlay mods[/b] — no swap-chain conflict, but both drive the pointer while their window is open, so expect contention there.[/*]
[*][b]Mod Engine 2[/b] — discontinued upstream, but the DLL is a plain native plugin.[/*]
[*][b]Spell or selection mods[/b] — direct conflict, both write the game's magic selection.[/*]
[*][b]Regulation mods[/b] — no conflict; this mod ships no param edits.[/*]
[/list]

[line]

[size=5][b]Notes[/b][/size]

[list]
[*]Removing the mod is a clean revert: delete the DLL and the profile, and the game is back to
vanilla. Nothing was added to your save file, and no game data was modified.[/*]
[*]Spell and weapon names are compiled into the DLL, so the mod works without unpacking anything.[/*]
[*]After a game update the mod falls back to the nearest known offset table and validates it before
touching memory; if validation fails it stays out of the way and says so in its log.[/*]
[*]If something misbehaves, the log in
[code]nr_casting\nr_casting_probe.log[/code]
records what the mod did, in order. Its
[code]modules:[/code]
line lists every DLL loaded besides Windows' own, with each DLL's load base, and any fatal fault
is recorded automatically as a [code]crash:[/code] line naming the exception, the address, the DLL
that owns it and the offset inside it — no debug logging needed first. Please attach those lines to
any bug report.[/*]
[*]If the DLL sits somewhere read-only, the mod writes to
[code]%APPDATA%\nr_casting\[/code]
instead; the log's
[code]files in:[/code]
line says which folder is in use.[/*]
[/list]

[line]

[size=4][b]Credits[/b][/size]

Built on [url=https://github.com/vswarte/fromsoftware-rs]fromsoftware-rs[/url] (structure and RVA definitions), [url=https://github.com/imgui-rs/imgui-rs]imgui-rs[/url] and [url=https://github.com/Yatekii/imgui-dx11-renderer]imgui-dx11-renderer[/url] for the panel.
Everything it reads about spells comes from your own copy of ELDEN RING.
