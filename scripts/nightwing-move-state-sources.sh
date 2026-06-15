#!/bin/sh
set -eu

controller_id_from_hidraw() {
  hidraw="$1"
  python3 - "$hidraw" <<'PY'
import fcntl
import os
import sys

path = sys.argv[1]
HIDIOCGFEATURE = lambda length: 0xC0000000 | (length << 16) | (0x48 << 8) | 0x07
try:
    fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
    buf = bytearray(16)
    buf[0] = 4
    fcntl.ioctl(fd, HIDIOCGFEATURE(len(buf)), buf, True)
    os.close(fd)
    print(''.join(f'{byte:02x}' for byte in reversed(buf[1:7])))
except Exception:
    try:
        os.close(fd)
    except Exception:
        pass
    sys.exit(1)
PY
}

hidraw_for_js() {
  js="$1"
  cursor="$(readlink -f "/sys/class/input/$(basename "$js")/device" 2>/dev/null || true)"
  while [ -n "$cursor" ] && [ "$cursor" != "/" ]; do
    if [ -d "$cursor/hidraw" ]; then
      for raw in "$cursor"/hidraw/hidraw*; do
        [ -e "$raw" ] || continue
        printf '/dev/%s\n' "$(basename "$raw")"
        return 0
      done
    fi
    cursor="$(dirname "$cursor")"
  done
  return 1
}

emit_candidates() {
for js in /dev/input/js*; do
  [ -e "$js" ] || continue
  props="$(udevadm info -q property -n "$js" 2>/dev/null || true)"
  uevent="/sys/class/input/$(basename "$js")/device/device/uevent"
  uevent_text="$(cat "$uevent" 2>/dev/null || true)"
  case "$props
$uevent_text" in
    *ID_VENDOR_ID=054c*ID_MODEL_ID=03d5*|*ID_MODEL=Motion_Controller*|*HID_ID=0005:0000054C:000003D5*|*HID_ID=0003:0000054C:000003D5*)
      ;;
    *)
      continue
      ;;
  esac

  bus="$(printf '%s\n' "$props" | awk -F= '$1=="ID_BUS"{print $2; exit}')"
  uniq="$(printf '%s\n' "$uevent_text" | awk -F= '$1=="HID_UNIQ"{print $2; exit}' | tr -d ':')"
  path_tag="$(printf '%s\n' "$props" | awk -F= '$1=="ID_PATH_TAG"{print $2; exit}' | tr -c 'A-Za-z0-9_' '_' | sed 's/_*$//')"
  hidraw="$(hidraw_for_js "$js" || true)"
  controller_id=""
  if [ -n "$hidraw" ]; then
    controller_id="$(controller_id_from_hidraw "$hidraw" 2>/dev/null || true)"
  fi

  if [ -n "$controller_id" ]; then
    id="move-$controller_id"
  elif [ "$bus" = "bluetooth" ] && [ -n "$uniq" ]; then
    id="move-$uniq"
  elif [ -n "$path_tag" ]; then
    id="move-usb-$path_tag"
  else
    id="move-$(basename "$js")"
  fi

  score=1
  case "$uevent_text" in
    *HID_ID=0005:0000054C:000003D5*) score=2 ;;
  esac

  printf '%s %s %s\n' "$id" "$score" "$js"
done
}

emit_candidates | awk '
  {
    id = $1
    score = $2 + 0
    path = $3
    if (!(id in best_score) || score >= best_score[id]) {
      best_score[id] = score
      best_path[id] = path
    }
  }
  END {
    for (id in best_path) {
      print id "=" best_path[id]
    }
  }
'
