#!/bin/sh
set -eu

host_address="${1:-}"
if [ -z "$host_address" ]; then
  host_address="$(bluetoothctl show | awk '/^Controller / { print $2; exit }')"
fi

python3 - "$host_address" <<'PY'
import fcntl
import os
import re
import sys

host_address = sys.argv[1].strip()
parts = re.split(r"[:-]", host_address)
if len(parts) != 6:
    print(f"invalid host address: {host_address}", file=sys.stderr)
    sys.exit(2)

try:
    host = bytes(int(part, 16) for part in reversed(parts))
except ValueError:
    print(f"invalid host address: {host_address}", file=sys.stderr)
    sys.exit(2)

HIDIOCGFEATURE = lambda length: 0xC0000000 | (length << 16) | (0x48 << 8) | 0x07
HIDIOCSFEATURE = lambda length: 0xC0000000 | (length << 16) | (0x48 << 8) | 0x06
MOVE_USB_ID = "HID_ID=0003:0000054C:000003D5"


def fmt_addr(raw):
    return ":".join(f"{byte:02x}" for byte in reversed(raw))


def is_usb_move(hidraw_name):
    uevent = f"/sys/class/hidraw/{hidraw_name}/device/uevent"
    try:
        with open(uevent, "r", encoding="utf-8", errors="replace") as stream:
            text = stream.read()
    except OSError:
        return False
    return MOVE_USB_ID in text


def read_addresses(fd):
    report = bytearray(16)
    report[0] = 0x04
    fcntl.ioctl(fd, HIDIOCGFEATURE(len(report)), report, True)
    return fmt_addr(report[1:7]), fmt_addr(report[10:16])


claimed = 0
errors = 0
for name in sorted(os.listdir("/sys/class/hidraw")):
    if not is_usb_move(name):
        continue

    path = f"/dev/{name}"
    try:
        fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
    except OSError as exc:
        errors += 1
        print(f"{path} open=failed error={exc}", file=sys.stderr)
        continue

    try:
        before_controller, before_host = read_addresses(fd)
        report = bytearray(23)
        report[0] = 0x05
        report[1:7] = host
        fcntl.ioctl(fd, HIDIOCSFEATURE(len(report)), report, True)
        after_controller, after_host = read_addresses(fd)
        claimed += 1
        print(
            f"{path} controller={after_controller or before_controller} "
            f"host_before={before_host} host_after={after_host}"
        )
    except OSError as exc:
        # Only the Move pairing collection accepts report 0x05. Other USB
        # collections are expected to reject it.
        print(f"{path} skipped error={exc}", file=sys.stderr)
    finally:
        os.close(fd)

if claimed == 0 and errors:
    sys.exit(1)
PY
