#!/usr/bin/env python3
"""Generate nr_casting_weapons.txt from a WitchyBND-unpacked ER regulation.

Equipped weapon ids carry the upgrade level in the low two digits (`33090010` is
`33090000` at +10), so only the base rows are stored here: the panel subtracts the
upgrade level before looking a name up.

Usage:
    ./gen-weapon-names.py regulation-bin/EquipParamWeapon.param.xml mod/data/nr_casting_weapons.txt

`regulation-bin/` is what WitchyBND writes when you unpack your own regulation.bin; it is
not committed. Drop the output beside the DLL to override the compiled-in table without a
rebuild, or regenerate mod/data/ and rebuild to bake it in.
"""
import re
import sys
import xml.etree.ElementTree as ET


def main(xml_path: str, out_path: str) -> int:
    rows = ET.parse(xml_path).getroot().findall("./rows/row")
    entries: list[tuple[int, str]] = []
    for row in rows:
        raw_id = row.get("id")
        if not raw_id:
            continue
        weapon_id = int(raw_id)
        name = (row.get("paramdexName") or "").strip()
        if not name:
            continue
        # Paramdex prefixes the class: "[Staff] Carian Regal Scepter", "[Dagger] Misericorde"
        name = re.sub(r"^\[[^\]]+\]\s*", "", name)
        entries.append((weapon_id, name))

    with open(out_path, "w", encoding="utf-8") as f:
        f.write("# base_weapon_id = Name   (upgrade level is the low two digits of the equipped id)\n")
        for weapon_id, name in sorted(entries):
            f.write(f"{weapon_id} = {name}\n")

    print(f"wrote {len(entries)} weapon names to {out_path}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        raise SystemExit(2)
    raise SystemExit(main(sys.argv[1], sys.argv[2]))
