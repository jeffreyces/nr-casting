# The chord modifier after the player rebinds it

Status: **proposal, awaiting the reporter's actual button list.** Nothing here is implemented; the
probe that produced the measurements below is in `input.rs`/`game.rs` as a debug line only.

## The report

> spells that require to hold y+rb/rt to cast only works when you have default control settings, so
> when i change buttons to my prefered control scheme, the only spells that are on attack buttons
> works. So the mod doesn't fully adapt to control changes in game settings, mainly the Event Action
> button.

Reproduced locally on build `2026-11-10T01:20:15Z` (0.2.24 + chord probe): with Event Action moved
off Y, the Y-chord slots do not fire and the plain shoulder slots still do.

## What was measured

One debug line in the frame clock next to the existing wheel observer, logging a transition of
`(physical Y bit, event_action, pouch)`. Evidence, in order, from a single session:

```
A  default bindings, Y held      : physical_y=true  event_action=true  pouch=true
B  Event Action moved to X       : physical_y=false event_action=true  pouch=true
C  default bindings, chord press : physical_y=true  event_action=false pouch=false
```

- **The action layer follows the rebind.** `CSPad::poll_digital_input(UserInputKey::EventAction)`
  resolves through `input_type_group` -> `key_assign.get_virtual_input_index()`, i.e. the player's own
  binding table. Our chord still tests the raw XInput bit `0x8000` (`XINPUT_GAMEPAD_Y`), which is only
  the _default_ mapping. Nothing corrupts: the chord simply never matches.
- **`EventActionPouch` (27) is not a second state.** It tracked `EventAction` (11) on every line, in
  both binding schemes. Drop it from the design.
- **The action layer cannot be the chord test.** Line C: during a chord we strip Y from the report so
  the game does not two-hand, and the action layer immediately reports `false` while the thumb is
  still down. Using the action poll would collapse the chord one frame after it starts.

## Options, and why each was rejected

| Option                                                     | Verdict                                                                                                                                                                                                                              |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Write the binding table to normalise it                    | **No.** Two writers on one binding table is corruption and an unrecoverable support story. Read-only, always.                                                                                                                        |
| Use `poll_digital_input(EventAction)` as the modifier test | **No.** Our own Y-strip makes the action false mid-chord (line C above).                                                                                                                                                             |
| Read `key_assign` / `input_type_group` by raw offset       | **Last resort.** Both accessors are private in the vendored bindings, so this is hardcoded offsets into the input system - the structure most likely to move between patches, and the one we would have to re-validate every update. |
| ini key naming the button (`chord_modifier`)               | **Fallback, not the answer.** Deterministic and offset-free, but it is a second place the player states a binding the game already knows, and it drifts again silently on the next rebind.                                           |
| **Learn the bit from the press edge** (proposed)           | Follows any rebind, no offsets, and the action layer acts as the check: a wrong guess never satisfies `event_action=true`.                                                                                                           |

## Proposed design (to refine)

Keep the chord test exactly as it is - a raw pad bit - but stop hardcoding _which_ bit.

1. Keep the whole raw button mask of the last report (`MODIFIER_HELD` today stores only the Y bit).
2. On the action layer's **rising edge** (`event_action` false -> true) with **exactly one** new bit in
   the report, that bit is the modifier. Cache it for the session.
3. Refuse to learn a bit the chord itself uses (`LB`, `RB`, both triggers) - if that is what the
   player bound, keep the ini value and log once.
4. Ambiguous press (two or more new bits, or no new bit): keep the current bit, log it once, learn on
   the next clean press.
5. `chord_modifier = auto | Y | X | B | A | LB | RB | LT | RT` in the ini. `auto` = learn; anything
   else pins the bit and disables learning. Live-reloadable, published to the panel.
6. The learned/pinned bit is printed in the load banner and in `state`, so a report says which button
   the session settled on.
7. Nothing is ever written to the game's binding table.

Out of scope: keyboard rebinds of Event Action (the keyboard path uses its own gesture), the class
port's chords, and any change to which slots exist.

## Tests to write first (in `nr_casting_core`, before any wiring)

| Behaviour                                                                 | Red before                      |
| ------------------------------------------------------------------------- | ------------------------------- |
| one new bit on the rising edge -> adopt it                                | yes                             |
| two new bits -> adopt nothing, keep current                               | yes                             |
| no new bit while the action rises -> keep current                         | yes                             |
| a candidate bit that the chord uses (LB/RB/LT/RT) -> refused              | yes                             |
| ini value other than `auto` -> learning never runs                        | yes                             |
| learned bit survives across presses in one session, resets on load        | yes                             |
| the modifier bit is stripped from the report only while the chord is ours | existing behaviour, re-asserted |

## Manual test script (needs the reporter's remap list first)

1. Default bindings: hold Y + R1 -> casts the fourth slot (regression check).
2. Move Event Action to the button the reporter uses; hold it + R1 -> must cast, and the banner must
   name the learned button.
3. Press the old Y + R1 -> must do the game's own action, not one of our slots.
4. Hold the new modifier alone -> must not cast anything, must not two-hand.
5. Two-button press (modifier + dpad) -> no mis-learning; the chord still works on the next press.
6. Restart, rebind again -> relearns without an ini edit.

## Risks

- **Mis-learning a bit** during a fast, multi-button press. Mitigated by the single-bit rule, the
  rising edge, the LB/RB/trigger refusal and the ini override.
- **A modifier on a shoulder** would collide with the chord itself; currently refused, needs a
  decision if the reporter does that.
- **Learning is per session**, so a report always shows the bit in use; persistence is a decision for
  later (an ini line, not a hidden cache).
- The debug line that measured all this stays debug-gated: it reads the pad manager every frame only
  when `debug_log = on`.

## Open questions for the reporter

1. Which buttons did they move, exactly - Event Action only, or the attack buttons too?
2. Pad only, or a keyboard-heavy scheme?
3. Is Event Action on a face button (X/B/A) or on a shoulder/trigger?
4. Does their scheme also move the spell-switch or pouch buttons? Those matter to the wheel skip.
