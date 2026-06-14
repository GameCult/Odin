#!/bin/sh
set -eu

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

  case "$bus:$uniq" in
    bluetooth:?*)
      id="move-bt-$uniq"
      ;;
    *)
      if [ -n "$path_tag" ]; then
        id="move-usb-$path_tag"
      else
        id="move-$(basename "$js")"
      fi
      ;;
  esac

  printf '%s=%s\n' "$id" "$js"
done
