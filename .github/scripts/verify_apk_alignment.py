#!/usr/bin/env python3
"""Verify all native lib entries in an APK are 4096-byte page-aligned.

Android's PackageInstaller rejects an APK (targetSdk 30+) whose stored
lib/ entries are not page-aligned with "App not installed". This must be
checked on the FINAL signed APK, because signing can re-pack the zip and
move offsets.
"""
import sys
import zipfile

ALIGN = 4096


def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} APK", file=sys.stderr)
        return 2
    apk = sys.argv[1]
    bad = []
    with zipfile.ZipFile(apk) as z:
        for info in z.infolist():
            if not info.filename.startswith("lib/"):
                continue
            off = info.header_offset + 30 + len(info.filename.encode("utf-8")) + len(info.extra)
            if off % ALIGN != 0:
                bad.append((info.filename, off))
    if bad:
        print("UNALIGNED native libs (offset % 4096 != 0):")
        for n, o in bad:
            print(f"  {n}: offset={o}")
        return 1
    print(f"OK: all native libs are {ALIGN}-byte aligned")
    return 0


if __name__ == "__main__":
    sys.exit(main())
