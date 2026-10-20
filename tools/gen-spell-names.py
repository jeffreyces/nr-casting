#!/usr/bin/env python3
"""Generate nr_casting_spell_names.txt from a WitchyBND-unpacked ER regulation.

Paramdex annotates every Magic row with `paramdexName`, so the spell names are
already in the converted XML — no FMG extraction, no encrypted BHD5, no Oodle.

Usage:
    ./gen-spell-names.py regulation-bin/Magic.param.xml mod/data/nr_casting_spells.txt

`regulation-bin/` is what WitchyBND writes when you unpack your own regulation.bin; it is
not committed. Drop the output beside the DLL to override the compiled-in table without a
rebuild, or regenerate mod/data/ and rebuild to bake it in.
"""
import re
import sys
import xml.etree.ElementTree as ET


def main(xml_path: str, out_path: str) -> int:
    rows = ET.parse(xml_path).getroot().findall("./rows/row")
    entries: list[tuple[int, str, str]] = []
    for row in rows:
        raw_id = row.get("id")
        if not raw_id:
            continue
        spell_id = int(raw_id)
        if spell_id >= 999999999:  # the dummy "no spell" row
            continue
        raw_name = (row.get("paramdexName") or "").strip()
        if not raw_name:
            continue
        # Paramdex prefixes the school: "[Sorcery] Comet", "[Incantation] Flame, Grant Me Strength"
        m = re.match(r"^\[([^\]]+)\]\s*(.*)$", raw_name)
        if m:
            school, name = m.group(1), m.group(2)
        else:
            school, name = "", raw_name
        # requirementLuck is Arcane (DS3 naming carried over)
        req = " | ".join(
            [
                school or "-",
                row.get("requirementIntellect") or "0",
                row.get("requirementFaith") or "0",
                row.get("requirementLuck") or "0",
                row.get("mp") or "0",
                row.get("slotLength") or "0",
            ]
        )
        entries.append((spell_id, name, req))

    with open(out_path, "w", encoding="utf-8") as f:
        f.write("# spell_id = Name | school | int | fai | arc | fp | slots\n")
        for spell_id, name, req in sorted(entries):
            f.write(f"{spell_id} = {name} | {req}\n")

    print(f"wrote {len(entries)} spell names to {out_path}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        raise SystemExit(2)
    raise SystemExit(main(sys.argv[1], sys.argv[2]))
