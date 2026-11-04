[center][size=6][b]Four Spells Per Catalyst[/b][/size]
[size=4]Eight spells on two staves (or seals) — with Elden Ring doing the casting[/size][/center]

[line]

[b]Four spells per staff/seal.[/b] R1/R2/Y+R1/Y+R2 cast the right-hand catalyst's four spells, L1/L2/Y+L1/Y+L2 the
left-hand one. That is eight castable spells across two catalysts, swapped instantly, without opening
the menu or touching the spell wheel mid-fight. For mouse/keyboard, you have native left/right + SHIFT/CTRL/ALT for a
total of eight spells.

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
either slot from the spells your character actually owns and can cast. A staff offers sorceries, a
seal incantations, and a catalyst built to cast both (the Staff of the Great Beyond, or a class
port's own) offers both.[/*]
[*][b]Balanced mode, if you want it[/b] — optional: only the spells you have memorised in the game's
own spell menu can be assigned, so the mod works within your memory slots instead of replacing
them.[/*]
[*][b]Per character[/b] — assignments are filed under the character's name, so the same catalyst on
two characters keeps two sets of spells (and a character is never handed spells it cannot cast).[/*]
[*][b]Nothing edited[/b] — no regulation changes, no save edits, no data files. Your assignments
live in a small text file next to the DLL, one per character.[/*]
[*][b]Safe to remove[/b] — no save file is ever written and nothing persists in memory, so the mod
can be uninstalled at any time and the game behaves exactly as it did before.[/*]
[*][b]Coexists with Recluse[/b] — that port uses Y+R1 / Y+L1 (and its 4 / 5 keys) for its own
actions; on a Recluse character those inputs go to it instead of casting the fourth spell.[/*]
[*][b]No anti-cheat, no network[/b] — it is loaded by me3, which starts the game with EAC disabled;
the mod itself never talks to anything.[/*]
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
Spells are assigned in the panel, or with the bind gesture: hold [b]L3[/b] and press the input while a
spell is selected on the game's own wheel, and that spell is written to the slot you pressed. On
keyboard and mouse hold the configured modifier (Super by default) with the same click.

[line]

[size=5][b]Requirements[/b][/size]
[list]
[*]ELDEN RING launched with EAC disabled (me3 does this). The mod does not support the official
online service and adds nothing to it.[/*]
[*][url=https://github.com/garyttierney/me3]me3[/url] (Mod Engine 3) to load the DLL.[/*]
[*]A controller for the pad bindings — the mod works at the XInput layer; mouse and keyboard work
without one.[/*]
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
overlay = on              ; show the panel at all
hotkey = F8               ; key that opens it
show_all = off            ; list spells you cannot cast yet
balance = free            ; free = any spell, attuned = only memorised ones
ui_scale = auto           ; auto follows the window, a number pins it
panel_size = auto         ; window size, saved when you stop dragging
panel_pos = auto          ; window corner, saved the same way
hud_mirror = off          ; force the wheel to slot 1

[input]
bind_modifier = L3        ; hold + a spell input to bind the wheel's spell
bind_modifier_kb = Super  ; the same gesture on mouse and keyboard
chord_stage = 2           ; reports spent releasing Y before a cast

[recluse]
y_bumper = auto           ; who gets Y+R1 / Y+L1: auto, on or off

[logging]
hooks = on                ; off = no input detours (comparison only)
debug_log = off           ; every cast logged
perf_log = on             ; a line on a lost frame, else every 5 min[/code]

Everything is optional: delete a line and the default above applies.

[code]balance = attuned[/code]
is the one worth knowing. Only spells you have memorised in the game's own spell menu can be
assigned, so the mod works within your memory slots instead of replacing them. A slot whose spell you
no longer have attuned is greyed rather than cleared — reassign it from its own dropdown, or re-attune
the spell and it comes back by itself.

[code]y_bumper = auto[/code]
takes [b]Y+R1[/b] / [b]Y+L1[/b] unless a class port is loaded and the character is one of its classes:
there the chord goes to the port instead, which uses [b]Y+RB[/b] / [b]Y+LB[/b] by default and hides
those buttons from the game while it does. Both mods want the same chord, so [code]auto[/code] yields
and that hand keeps three spells rather than four.
[code]on[/code]
always takes them,
[code]off[/code]
gives vanilla two-handing back.

To get the fourth slot on a class character, set [code]y_bumper = on[/code] and move the class skills
off the chord in [b]Options > Button Settings > Origin Skills[/b] ([b]X+RB[/b] / [b]X+LB[/b] is the
usual choice). The port reads that menu, so it follows you. The mod cannot make this choice for you:
where you put Origin Skills lives in the game's own button settings and cannot be read.

[code]hooks = off[/code]
installs nothing and casting stops; it exists only to measure the game with the DLL loaded and idle.

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
[*][b]me3[/b] — supported, including me3 Manager. [b]Vortex[/b] also works: it deploys the DLL like any
native mod, and the shipped [code].me3[/code] profile is only needed for me3.[/*]
[*][b]Frame-generation and upscaling mods[/b] (FSR/FG) [url=https://www.patreon.com/huutaiii/posts/erss-fg-free-v4-125972766]https://www.patreon.com/huutaiii/posts/erss-fg-free-v4-125972766[/url]:
[code]d3d12.dll[/code]
[code]dxgi.dll[/code]
[code]erss-fg.dll[/code]
 — the panel is a separate window, so it draws regardless of who presents the frame. Casting is unaffected either way.[/*]
[*][b]Class ports with their own HUD[/b] (Recluse and friends) [url=https://www.nexusmods.com/eldenring/mods/11410]Nightreign Recluse Full Port[/url]:
[code]ERNativeUI.dll[/code]
 — no conflict: the panel never touches the game's swap chain, so their on-screen HUD keeps drawing.[/*]
[*][b][url=https://www.nexusmods.com/eldenring/mods/510]Seamless Co-op[/url][/b] — runs its own launcher with EAC off, and casting has been reported
working alongside it. The mod is not involved in the co-op layer; every player in a session still
needs Seamless Co-op itself.[/*]
[*][b]ERD Tools[/b] — supported. Earlier builds crashed inside ERD a second after the panel opened;
measured cause was the panel's own swap chain and [code]Present[/code] (ERD hooks that path
process-wide), so the panel no longer has a swap chain: it renders to a texture, copies to a DIB and
[code]BitBlt[/code]s it, calling no [code]Present[/code] at all.[/*]
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
[*]Spell and weapon names come from your own game's text at load (a few thousand names, about
20 ms), so nothing is unpacked and no name list ships with the DLL.[/*]
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
that owns it, the offset inside it and the callers behind it — no debug logging needed first. Please attach those lines to
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
