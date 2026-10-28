[center][size=6][b]Four Spells Per Catalyst[/b][/size]
[size=4]Eight spells on two staves — with Elden Ring doing the casting[/size][/center]

[line]

[b]Four spells per staff.[/b] R1/R2/Y+R1/Y+R2 cast the right-hand catalyst's two spells, L1/L2/Y+L1/Y+L2 the
left-hand one. That is four castable spells across two staves, swapped instantly, without opening
the menu or touching the spell wheel mid-fight. For mouse/keyboard, you have native left/right + SHIFT/CTRL/ALT for a 
total of four spells.

This is not a custom casting implementation. The mod rewrites the controller buttons reports into shoulder
presses before the game reads the pad, so Elden Ring runs its own cast: real FP costs, real
requirements, real animations, real i-frames. If the game would not let you cast it, the mod will
not either.

[line]

[size=5][b]Features[/b][/size]
[list]
[*][b]Eight spells, two catalysts[/b] — a bumper spell and a trigger spell per hand, a third on
[b]Y+R2[/b] / [b]Y+L2[/b] and a fourth on [b]Y+R1[/b] / [b]Y+L1[/b]; two-handing a staff does nothing, so those inputs carry spells instead.[/*]
[*][b]In-game assignment[/b] — a panel in its own window lists your equipped catalysts and lets you pick
either slot from the spells your character actually owns and can cast.[/*]
[*][b]Per character[/b] — assignments are filed under the character's name, so the same catalyst on
two characters keeps two sets of spells (and a character is never handed spells it cannot cast).[/*]
[*][b]Nothing edited[/b] — no regulation changes, no save edits, no data files. Your assignments
live in a small text file next to the DLL, one per character.[/*]
[*][b]Safe to remove[/b] — no save file is ever written and nothing persists in memory, so the mod
can be uninstalled at any time and the game behaves exactly as it did before.[/*]
[*][b]Coexists with Recluse[/b] — that port uses Y+R1 / Y+L1 (and its 4 / 5 keys) for its own
actions; on a Recluse character those inputs go to it instead of casting the fourth spell.[/*]
[*][b]Offline only[/b] — built for me3 with online play disabled, so EAC never loads.[/*]
[/list]

[line]

[size=5][b]Controls[/b][/size]
[list]
[*][b]F8[/b] — open/close the assignment panel[/*]
[*][b]R1 / R2 / Y + R1 / Y + R2[/b] — cast slots 1-4 of the right-hand catalyst[/*]
[*][b]L1 / L2 / Y + L1 / Y + L2[/b] — cast slots 1-4 of the left-hand catalyst[/*]
[*][b]Mouse[/b] — click, shift + click, ctrl + click, alt + click (left or right button) for the same eight slots, with or without a controller[/*]
[/list]

The panel hotkey is configurable in
[code]nr_casting.ini[/code]
Spells are assigned in the panel; there is no bind gesture on the pad.

[line]

[size=5][b]Requirements[/b][/size]
[list]
[*]ELDEN RING, offline. Online play and EAC are not supported and not tested.[/*]
[*][url=https://github.com/garyttierney/me3]me3[/url] (Mod Engine 3) to load the DLL.[/*]
[*]A controller — the mod works at the XInput layer.[/*]
[*]Keyboard  / Mouse — Also works with Left/Right click (original) But added SHIFT / CTRL / ALT + Left / Right to reach full 8.[/*]
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
show_all = off

[recluse]
y_bumper = auto
recluse_chr_ids = 8131

[logging]
debug_log = off[/code]

[code]y_bumper[/code]
decides who gets [b]Y+R1[/b] / [b]Y+L1[/b]:
[code]auto[/code]
takes them unless the Recluse mod is
loaded and the character is a Recluse — there the chord goes to the class port's own
[b]4[/b] / [b]5[/b] keys (the pad report is left alone; an Alt click is translated into that key) —
[code]on[/code]
always takes them,
[code]off[/code]
gives vanilla two-handing back.

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
[*][b][url=https://www.nexusmods.com/eldenring/mods/510]Seamless Co-op[/url] — [/b]Utilizing this allows for online play with friends. EAC is still off. According to docs, friends also need this mod if one person has it.[/*]
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
is recorded automatically as a
[code]crash:[/code]
line naming the exception, the address, the DLL
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
