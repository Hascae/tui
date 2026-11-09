#!/usr/bin/env python3
"""Pack the Big5-UAO 2.50 mapping tables into the binary tables the core embeds.

Source tables: https://moztw.org/docs/big5/table/uao250-b2u.txt and uao250-u2b.txt
Usage: gen_uao.py <b2u.txt> <u2b.txt> <out-dir>
"""
import struct
import sys

LEAD_MIN, LEAD_MAX = 0x81, 0xFE
TRAIL_MIN, TRAIL_MAX = 0x40, 0xFE
TRAILS = TRAIL_MAX - TRAIL_MIN + 1


def pairs(path):
    with open(path, encoding="ascii") as f:
        for line in f:
            if line.startswith("0x"):
                a, b = line.split()[:2]
                yield int(a, 16), int(b, 16)


def main():
    b2u_path, u2b_path, out = sys.argv[1:4]

    b2u = [0] * ((LEAD_MAX - LEAD_MIN + 1) * TRAILS)
    for big5, uni in pairs(b2u_path):
        lead, trail = big5 >> 8, big5 & 0xFF
        if not (LEAD_MIN <= lead <= LEAD_MAX and TRAIL_MIN <= trail <= TRAIL_MAX):
            continue
        b2u[(lead - LEAD_MIN) * TRAILS + (trail - TRAIL_MIN)] = uni

    u2b = [0] * 0x10000
    for big5, uni in pairs(u2b_path):
        if big5 >= 0x8140 and big5 != 0xFFFD:
            u2b[uni] = big5

    with open(f"{out}/b2u.bin", "wb") as f:
        f.write(struct.pack(f"<{len(b2u)}H", *b2u))
    with open(f"{out}/u2b.bin", "wb") as f:
        f.write(struct.pack(f"<{len(u2b)}H", *u2b))
    print(sum(1 for v in b2u if v), "b2u entries,", sum(1 for v in u2b if v), "u2b entries")


if __name__ == "__main__":
    main()
