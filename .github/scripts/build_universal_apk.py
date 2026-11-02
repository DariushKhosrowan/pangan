#!/usr/bin/env python3
"""Merge per-ABI Pangan APKs into one universal (fat) APK.

Builds a standards-compliant fat APK using the stdlib `zipfile` writer.
Native libs are stored uncompressed and 4K-aligned via a proper extra field
(the same layout zipalign produces), which is required for targetSdk 30+
installations.

Output is an *unsigned* aligned APK; sign it afterwards with apksigner
(v1+v2+v3).

Usage:
    build_universal_apk.py OUT.apk IN1.apk IN2.apk [IN3.apk ...]
"""
import sys
import os
import zipfile
import tempfile

ALIGN = 4096


def _load(path):
    with zipfile.ZipFile(path) as z:
        return {n: z.read(n) for n in z.namelist()}


def _lib_offsets(path, lib_names):
    """Return {name: data_start_offset} for each stored lib entry in `path`."""
    result = {}
    pos = 0
    with zipfile.ZipFile(path) as z:
        for info in z.infolist():
            data_start = pos + 30 + len(info.filename.encode("utf-8")) + len(info.extra)
            if info.filename in lib_names:
                result[info.filename] = data_start
                pos = data_start + info.file_size
            else:
                pos = data_start + info.compress_size
    return result


def write_apk(path, entries, pad):
    with zipfile.ZipFile(path, "w") as zout:
        for name, data in entries:
            info = zipfile.ZipInfo(name)
            info.compress_type = (zipfile.ZIP_STORED
                                  if name.startswith("lib/")
                                  else zipfile.ZIP_DEFLATED)
            info.date_time = (1980, 1, 1, 0, 0, 0)
            if name.startswith("lib/") and pad.get(name):
                info.extra = b"\x00" * pad[name]
            zout.writestr(info, data)


def build(out_path, src_paths):
    first = _load(src_paths[0])
    libs = {}
    for p in src_paths:
        for n, d in _load(p).items():
            if n.startswith("lib/") and n not in libs:
                libs[n] = d

    non_lib = [(n, d) for n, d in first.items()
               if not n.startswith("lib/") and not n.startswith("META-INF/")]
    entries = non_lib + sorted(libs.items())
    lib_names = set(libs)

    # Iteratively align libs one at a time in order. Fixing lib i adds
    # padding after lib i-1, which only shifts libs AFTER i — so once i is
    # aligned it stays aligned. This terminates in <= len(libs) rebuilds.
    pad = {}
    write_apk(out_path, entries, pad)
    for _ in range(len(lib_names) + 1):
        offs = _lib_offsets(out_path, lib_names)
        target = next((n for n in sorted(lib_names)
                       if offs[n] % ALIGN != 0), None)
        if target is None:
            break
        pad[target] = (ALIGN - (offs[target] % ALIGN)) % ALIGN
        write_apk(out_path, entries, pad)
    else:
        sys.exit("could not align native libs")

    offs = _lib_offsets(out_path, lib_names)
    if any(offs[n] % ALIGN != 0 for n in lib_names):
        sys.exit(f"alignment failed: {offs}")

    with zipfile.ZipFile(out_path) as zcheck:
        if zcheck.testzip() is not None:
            sys.exit("integrity error in rebuilt APK")
    print(f"built {out_path}: {len(entries)} entries, {len(libs)} abi libs, "
          f"aligned: {sorted(lib_names)}")


if __name__ == "__main__":
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    out = sys.argv[1]
    srcs = sys.argv[2:]
    for s in srcs:
        if not os.path.exists(s):
            print(f"missing {s}", file=sys.stderr)
            sys.exit(1)
    build(out, srcs)
