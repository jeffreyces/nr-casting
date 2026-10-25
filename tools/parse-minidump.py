#!/usr/bin/env python3
"""Read a Windows minidump without WinDbg: the exception, and the stack around it.

    tools/parse-minidump.py <file.dmp> [--stack-bytes N]

Prints the exception code, the faulting address resolved to module+offset, whether an access
violation was a read or a write, and every stack word that points into a loaded module. Good
enough to answer "which module faulted, and what called it" from a user's crash dump.

Only the streams it needs are parsed: SystemInfo is skipped, ThreadList is used for the stack
range, ModuleList for names, MemoryList for the bytes. The frame walk is a raw pointer scan, not
a real unwind (no unwind info in most game dumps), so treat the order as a hint, not a backtrace.
"""
import struct
import sys

IGNORED_CODES = {
    0x406D1388: "MS_VC_EXCEPTION (thread name)",
    0xE06D7363: "MSVC C++ throw",
    0x40010006: "OutputDebugString",
    0x4001000A: "OutputDebugStringW",
    0x80000003: "breakpoint",
}


class Dump:
    def __init__(self, path):
        self.f = open(path, "rb")
        sig, _ver, nstreams, dir_rva, _cksum, _flags = struct.unpack("<IIIIII", self.f.read(24))
        if sig != 0x504D444D:
            raise SystemExit("not a minidump (bad signature)")
        self.f.seek(dir_rva)
        self.streams = {}
        for _ in range(nstreams):
            stype, size, rva = struct.unpack("<III", self.f.read(12))
            self.streams[stype] = (size, rva)
        self.modules = self._modules()
        self.threads = self._threads()

    def read(self, rva, size):
        self.f.seek(rva)
        return self.f.read(size)

    def _modules(self):
        base_rva = self.streams[4][1]
        n = struct.unpack("<I", self.read(base_rva, 4))[0]
        out = []
        for i in range(n):
            rec = self.read(base_rva + 4 + i * 108, 24)
            base, size, _cksum, _ts, name_rva = struct.unpack("<QIIII", rec)
            ln = struct.unpack("<I", self.read(name_rva, 4))[0]
            name = self.read(name_rva + 4, ln).decode("utf-16-le", "replace").rstrip("\x00")
            out.append((base, size, name))
        return out

    def _threads(self):
        rva = self.streams[3][1]
        n = struct.unpack("<I", self.read(rva, 4))[0]
        out = []
        for i in range(n):
            rec = self.read(rva + 4 + i * 48, 48)
            tid, _susp, _prio, _pad, _teb, ss, ssz, srva, _cs, _crva = struct.unpack(
                "<IIIIQQIIII", rec
            )
            out.append((tid, ss, ssz, srva))
        return out

    def resolve(self, addr):
        for base, size, name in self.modules:
            if base <= addr < base + size:
                return name.split("\\")[-1], addr - base
        return None, None

    def exception(self):
        size, rva = self.streams[6]
        raw = self.read(rva, size)
        tid = struct.unpack("<I", raw[0:4])[0]
        code, _flags, _rec, addr, nparams, _unused = struct.unpack("<IIQQII", raw[8:40])
        params = struct.unpack("<15Q", raw[40:160])
        csize, crva = struct.unpack("<II", raw[160:168])
        ctx = self.read(crva, csize)
        rsp = struct.unpack("<Q", ctx[0x98:0xA0])[0]
        rip = struct.unpack("<Q", ctx[0xF8:0x100])[0]
        return {
            "tid": tid,
            "code": code,
            "address": addr,
            "params": params[:nparams],
            "rsp": rsp,
            "rip": rip,
        }

    def stack_words(self, rsp, limit):
        """Return (addr, word) for each stack slot, using the memory descriptor that covers rsp."""
        rva = self.streams[5][1]
        n = struct.unpack("<I", self.read(rva, 4))[0]
        for i in range(n):
            start, dsize, drva = struct.unpack("<QII", self.read(rva + 4 + i * 16, 16))
            if start <= rsp < start + dsize:
                data = self.read(drva + (rsp - start), min(limit, start + dsize - rsp))
                return [
                    (rsp + off, struct.unpack("<Q", data[off : off + 8])[0])
                    for off in range(0, len(data) - 7, 8)
                ]
        return []


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    path = sys.argv[1]
    limit = 0x400
    if "--stack-bytes" in sys.argv:
        limit = int(sys.argv[sys.argv.index("--stack-bytes") + 1])

    d = Dump(path)
    ex = d.exception()
    mod, off = d.resolve(ex["address"])
    what = IGNORED_CODES.get(ex["code"])
    print(f"exception : {ex['code']:#010x}" + (f"  ({what}, usually not fatal)" if what else ""))
    print(f"thread    : {ex['tid']}")
    print(f"address   : {ex['address']:#x}  ->  {mod}+{off:#x}" if mod else f"address   : {ex['address']:#x}")
    if ex["code"] == 0xC0000005 and len(ex["params"]) >= 2:
        op = {0: "read", 1: "write", 8: "execute"}.get(ex["params"][0], ex["params"][0])
        print(f"access    : {op} of {ex['params'][1]:#x}")
    print(f"rsp/rip   : {ex['rsp']:#x} / {ex['rip']:#x} ({d.resolve(ex['rip'])[0] or '?'})")
    print("stack     :")
    for addr, word in d.stack_words(ex["rsp"], limit):
        name, rva = d.resolve(word)
        if name:
            print(f"  {addr:#x}  {word:#x}  {name}+{rva:#x}")


if __name__ == "__main__":
    main()
