# Facts measured from the running game

Everything here was read out of the live process, not inferred from wiki text or paramdex names.
Each section says how it was measured, so it can be re-measured rather than re-guessed after a patch.

## Button bindings are resolved by the game, not by the pad report

**The game owns the mapping; the XInput bit is only the default.** `CSPad::poll_digital_input` takes a
`UserInputKey` (`EventAction = 11`, `EventActionPouch = 27`, `Attack = 7`, ...) and resolves it through
`input_type_group` -> `key_assign.get_virtual_input_index(mapped_input)` - the player's own binding
table - so the action layer follows a rebind to any pad button or key. Reached with
`FD4PadManager::instance()?.get_in_game_pad()`.

Measured 2026-11-10 on build `01:20:15Z` (debug line `chord:`), three cases in one session:

| case | physical Y bit | `event_action` | `pouch` |
|---|---|---|---|
| default bindings, Y held | true | true | true |
| Event Action moved to X | false | true | true |
| default bindings, chord press | true | false | false |

Facts that follow, and that a design has to respect:

- `EventActionPouch` is **not** a second state - it tracked `EventAction` on every line in both
  binding schemes.
- The action layer reports what the game **sees**, not what the player did: stripping Y from the
  report (which this mod does during a chord so the game does not two-hand) makes `event_action`
  false while the button is still held. So the action poll cannot be used as the chord's modifier
  test.
- `input_type_group` and `key_assign` are private fields in the vendored bindings; reading them means
  raw offsets into the input system. Prefer behaviour that needs neither.

## Equipped weapons (`PlayerGameData.equipment_param_ids`)

**All six carried weapons are readable, three per hand.** `chr_asm.equipment_param_ids` is a 22-entry
`i32` array and slots `0..=5` are `ChrAsmSlot` 0–5 — `WeaponLeft1, WeaponRight1, WeaponLeft2,
WeaponRight2, WeaponLeft3, WeaponRight3`. `active_weapon_slot(hand)` separately says which of them is
in a hand right now; that is the cast path's question, and the wheel's question is the whole carried
set (a spell bound to the spare staff is still spoken for).

Source of the mapping: the bindings' own documentation for `ChrAsmSlot`, not a measurement of ours.
The engine prints the six ids labelled (`spell: carried L1=… R1=… L2=… R3=…`) at panel open so a
report can confirm them against the equipment screen; if that line ever disagrees with what the player
carries, this is the section to correct.

## Catalyst spell school (`EquipParamWeapon`)

**A catalyst's schools are two flag bits in its own row.** The Paramdex-generated bindings name every
field of `EQUIP_PARAM_WEAPON_ST` and none of them says "spell school" — `weapon_category`,
`wepmotion_category`, `wep_type`, `behavior_variation_id`, `sp_attribute` are all about how a weapon
swings. The flag is in two bytes the paramdef leaves unlabelled.

| offset | field | bit | means |
|---|---|---|---|
| `0x105` | `bits_105` | `0x80` | casts sorceries |
| `0x106` | `bits_106` | `0x02` | casts incantations |

Measured row size: **664 bytes** (`std::mem::size_of_val` on the row returned by
`SoloParamRepository::get::<EquipParamWeapon>`). The bindings generate `bits_105` / `bits_106` **without
accessors**, so they can only be read by offset — which is why `params::catalyst_flags` validates the
offsets once per process against a known staff and a known seal before trusting them.

Values observed (game build 2.7.2.0, 2026-11-04):

| weapon | id | bits_105 | bits_106 | casts |
|---|---|---|---|---|
| 12 vanilla staves (`33000000` … `33240000`) | — | `0x87` | `0x10` | sorceries |
| 11 vanilla seals (`34000000` … `34510000`) | — | `0x07` | `0x12` | incantations |
| Staff of the Great Beyond (SotE) | `33510000` | `0x87` | `0x12` | both |
| Recluse port's staff | `33790000` | `0x87` | `0x12` | both |
| Prince of Death's Staff | `33180000` | `0x87` | `0x10` | sorceries only |

The last row is the control that matters: INT/FAI scaling is a separate set of fields
(`correct_magic`, `correct_faith`, `correct_type_magic`, …) and says nothing about what the weapon can
cast. `33180000` scales with both stats and still casts sorceries only.

Two independent mods landing on the same two bits (the DLC staff and the port's staff) is what made
this trustworthy; single-school staves and seals differ by exactly one bit each.

### How it was measured

A temporary probe logged the raw bytes of 25 weapon rows (`catalyst_flags` dump, since removed):

```rust
let row = repo.get::<EquipParamWeapon>(id)?;                 // marker type; size comes from the value
let size = std::mem::size_of_val(row);
let bytes = std::slice::from_raw_parts(row as *const _ as *const u8, size);
```

then diffed: staves vs seals gives the per-school bits, and the dual catalysts show both. `size_of::<T>()`
on the *marker* type is 0, which is the trap to remember.

## Character class (Recluse / Duchess class ports)

**The class is not readable from the player structures on current builds.** Measured on a port
character (`world: class probe`, since removed):

```
chr_type=0 archetype=10 voice=6 gender=1 character_id=0 player_id=0 npc_id=0 npc_param_id=0 chr_type=0 npc_param=false
```

- `PlayerGameData`: `character_event_id`, `character_id`, `player_id`, `chr_type` all **0**.
- `ChrIns`: `character_id`, `npc_id`, `npc_param_id`, `chr_type` all **0**; `npc_param` pointer absent.
- The only non-zero fields — `archetype`, `voice_type`, `gender` — are character-creation choices, not
  class: any class can pick any of them.
- `PlayerGameData.character_name` **is** readable (the mod prints it), so the structure is there; the
  class simply is not one of its fields.

A port class exists as a custom chr asset (`c8131.chrbnd`, …). Naming it would mean reading the model
instance (`CSFD4ModelItem`, mostly unmarked memory) and matching an asset string — one more offset to
validate per patch, for a label.

**What is used instead:** the port weapon in hand, via a shipped table
(`mod/data/nr_casting_class_ports.txt`: `33790000 = Recluse`, `33700000`/`33700800` = `Duchess`). It is
exact whenever the port's own weapon is equipped, user-extensible, and degrades to "class port".
The same signal (a catalyst missing from the vanilla weapon table) is what detects "this is a port
character" at all, and it needs no configuration.

## Rules this produced

- **Probe first.** If a value is needed and its location is not already measured, read it from the live
  process and diff two known-good cases — do not infer it from field names, and do not ship a hardcoded
  table instead of a read. A table is a *fallback* for when the read cannot be validated, never the
  first answer.
- **Validate any offset read** against a known row before trusting it for everything else.
- **Name the mechanism in the log** ("read from the weapon rows", "did not validate — using the id
  band") so a report says which source answered.
