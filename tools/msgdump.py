#!/usr/bin/env python3
"""Dump every FMG name from an Elden Ring .msgbnd.dcx without Windows or Oodle.

Pipeline: DCX header -> Kraken decode (ooz build: krakenchunks) -> BND4 file table
(name per sub-file, e.g. WeaponName.fmg) -> FMG ranges + 8-byte string offsets.

Why this exists: item names are not in regulation.bin or the params — they are FMG
text, and a mod that adds items ships loose `msg/<lang>/*.msgbnd.dcx` replacements.
Those are DCX/KRAK (Oodle), which is why `grep` finds nothing and "the names are
locked behind the archive" is only half true: the archive is encrypted, a mod's
override is not, it is merely Oodle-compressed.

Usage:
    msgdump.py <file.msgbnd.dcx> <out.json> [--kraken PATH]

KRAK half needs `krakenchunks` (ooz Kraken decoder, own build). DFLT (zlib) works
with no helper. Set KRAKEN_CHUNKS or pass --kraken.
"""
from __future__ import annotations

import json
import os
import struct
import subprocess
import sys
import tempfile
import zlib


def be32(b: bytes, o: int) -> int:
    return struct.unpack(">I", b[o:o + 4])[0]


def dcx_payload(path: str, kraken: str) -> bytes:
    d = open(path, "rb").read()
    if d[:4] != b"DCX\0":
        return d
    dcs = be32(d, 8)
    data_off = be32(d, 20)
    usize = be32(d, dcs + 4)
    csize = be32(d, dcs + 8)
    comp = d[dcs + 16:dcs + 20]
    if comp == b"DFLT":
        return zlib.decompress(d[data_off:data_off + csize])
    if comp != b"KRAK":
        raise SystemExit(f"unsupported DCX compression {comp!r}")
    if not kraken or not os.path.exists(kraken):
        raise SystemExit("KRAK payload needs the krakenchunks helper (--kraken / KRAKEN_CHUNKS)")
    with tempfile.NamedTemporaryFile(suffix=".dcx", delete=False) as f:
        f.write(d)
        tmp = f.name
    out = tmp + ".bin"
    try:
        subprocess.run([kraken, tmp, str(data_off), str(len(d) - data_off), str(usize), out],
                       check=True, stdout=subprocess.DEVNULL)
        return open(out, "rb").read()
    finally:
        for p in (tmp, out):
            try:
                os.unlink(p)
            except OSError:
                pass


def parse_bnd4(d: bytes):
    """-> [(group_id, internal_name, offset, size)] for a BND4 with 36-byte file headers."""
    if d[:4] != b"BND4":
        raise SystemExit("not BND4")
    count = struct.unpack("<I", d[0x0C:0x10])[0]
    subs = []
    for k in range(count):
        o = 0x40 + k * 36
        size, _usize = struct.unpack("<qq", d[o + 8:o + 24])
        off, group = struct.unpack("<II", d[o + 24:o + 32])
        name_off = struct.unpack("<I", d[o + 32:o + 36])[0]
        end = name_off
        while end + 1 < len(d) and not (d[end] == 0 and d[end + 1] == 0):
            end += 2
        name = d[name_off:end].decode("utf-16-le", "replace").replace("\\", "/").rsplit("/", 1)[-1]
        subs.append((group, name, off, size))
    return subs


def parse_fmg(d: bytes):
    """-> {text_id: string}. Accepts the FMG body with or without the 'FMG ' magic."""
    if d[:4] == b"FMG ":
        d = d[4:]
    range_count, string_count = struct.unpack("<II", d[12:20])
    soff = struct.unpack("<Q", d[24:32])[0]
    pos = 40
    ranges = []
    for _ in range(range_count):
        first_index, first_id, last_id = struct.unpack("<iii", d[pos:pos + 12])
        pos += 16
        ranges.append((first_index, first_id, last_id))
    # FMG v2 ("long varints") stores string offsets as 8-byte values, not u32.
    offs = struct.unpack(f"<{string_count}Q", d[soff:soff + string_count * 8])
    out = {}
    for first_index, first_id, last_id in ranges:
        if last_id < first_id:
            continue
        for sid in range(first_id, last_id + 1):
            o = offs[first_index]
            first_index += 1
            if o == 0 or o + 1 >= len(d):
                out[sid] = ""
                continue
            end = o
            while end + 1 < len(d) and not (d[end] == 0 and d[end + 1] == 0):
                end += 2
            out[sid] = d[o:end].decode("utf-16-le", "replace")
    return out


def main(argv):
    args = [a for a in argv[1:] if not a.startswith("--")]
    if len(args) < 2:
        print(__doc__)
        return 2
    kraken = os.environ.get("KRAKEN_CHUNKS", "")
    if "--kraken" in argv:
        kraken = argv[argv.index("--kraken") + 1]

    raw = dcx_payload(args[0], kraken)
    groups = {}
    for group, name, off, size in parse_bnd4(raw):
        entries = {str(k): v for k, v in parse_fmg(raw[off:off + size]).items()}
        groups[str(group)] = {"name": name, "entries": entries}

    with open(args[1], "w", encoding="utf-8") as f:
        json.dump(groups, f, ensure_ascii=False, indent=0)

    print(f"{len(groups)} groups -> {args[1]}")
    for g, m in sorted(groups.items(), key=lambda kv: int(kv[0])):
        named = sum(1 for v in m["entries"].values() if v)
        print(f"  group {g:>3} {m['name']:<20} {len(m['entries'])} ids, {named} named")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
