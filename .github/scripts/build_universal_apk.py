#!/usr/bin/env python3
"""Merge per-ABI Pangan APKs into one universal (fat) APK.

Native libs are stored uncompressed and 4K-aligned (page alignment for API 30+
and 16K-page devices). Non-lib entries are taken from the first APK (the
manifest is ABI-independent). Output is an *unsigned* aligned APK; sign it
afterwards with apksigner (v1+v2+v3).

Usage:
    build_universal_apk.py OUT.apk IN1.apk IN2.apk [IN3.apk ...]
"""
import sys, zipfile, struct, binascii, subprocess, os

ALIGN = 4096

def load_entries(path):
    with zipfile.ZipFile(path) as z:
        return {n: z.read(n) for n in z.namelist()}

def build(out_path, src_paths):
    # union of all member names; manifest/etc from first apk
    first = load_entries(src_paths[0])
    libs = {}
    for p in src_paths:
        for n, d in load_entries(p).items():
            if n.startswith("lib/") and n not in libs:
                libs[n] = d
    # entry order: manifest first, then libs (libs after so they get 4K offsets)
    non_lib = [(n, d) for n, d in first.items()
               if not n.startswith("lib/") and not n.startswith("META-INF/")]
    lib_items = sorted(libs.items())
    entries = non_lib + lib_items  # (name, data)

    def write_entry(f, name, data, align=None):
        crc = binascii.crc32(data) & 0xffffffff
        stored = name.startswith("lib/")
        if stored:
            method = zipfile.ZIP_STORED
            cdata, cs = data, len(data)
        else:
            method = zipfile.ZIP_DEFLATED
            co = zipfile._get_compressor(method)
            cdata = co.compress(data) + co.flush(); cs = len(cdata)
        nb = name.encode("utf-8")
        tm, dt = 0x0000, 0x0021
        extra = 0
        if stored and align:
            base = 30 + len(nb)
            extra = (align - ((f.tell() + base) % align)) % align
        f.write(struct.pack("<IHHHHHIIIHH", 0x04034b50, 20, 0, method,
                            tm, dt, crc, cs, len(data), len(nb), extra))
        f.write(nb)
        if extra: f.write(b"\x00" * extra)
        f.write(cdata)
        return (nb, method, tm, dt, crc, cs, len(data), 0, 0, 0, 0,
                f.tell() - len(cdata) - extra - len(nb) - 30)

    with open(out_path, "wb") as f:
        central = []
        for name, data in entries:
            central.append(write_entry(f, name, data, ALIGN if name.startswith("lib/") else None))
        cd_start = f.tell()
        for (nb, method, tm, dt, crc, cs, n, elen, clen, disk, iattr, off) in central:
            f.write(struct.pack("<IHHHHHHIIIHHHHHII", 0x02014b50, 20, 20, 0,
                                method, tm, dt, crc, cs, n,
                                len(nb), elen, clen, disk, iattr, 0, off))
            f.write(nb)
        cd_size = f.tell() - cd_start
        f.write(struct.pack("<IHHHHIIH", 0x06054b50, 0, 0,
                            len(central), len(central), cd_size, cd_start, 0))
    subprocess.run(["unzip", "-t", out_path], check=True, capture_output=True)
    print(f"built {out_path}: {len(entries)} entries, {len(libs)} abi libs")

if __name__ == "__main__":
    if len(sys.argv) < 4:
        print(__doc__); sys.exit(2)
    out = sys.argv[1]; srcs = sys.argv[2:]
    for s in srcs:
        if not os.path.exists(s):
            print(f"missing {s}", file=sys.stderr); sys.exit(1)
    build(out, srcs)
