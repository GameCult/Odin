#!/bin/sh
set -eu

psmove_bin=/usr/local/bin/psmove
adapter_address="$(bluetoothctl show | awk '/^Controller / { print toupper($2); exit }')"
if [ ! -x "$psmove_bin" ] || [ -z "$adapter_address" ]; then
  echo "PSMoveAPI or Nightwing Bluetooth adapter unavailable" >&2
  exit 1
fi

missing=0
controllers="$($psmove_bin list 2>/dev/null | grep -Eio '[0-9a-f]{2}(:[0-9a-f]{2}){5}' | sort -u)"
for controller in $controllers; do
  controller_upper="$(printf '%s' "$controller" | tr '[:lower:]' '[:upper:]')"
  cache="/var/lib/bluetooth/$adapter_address/cache/$controller_upper"
  if [ ! -s "$cache" ] || ! grep -q '^\[ServiceRecords\]$' "$cache"; then
    missing=1
    break
  fi
done

if [ "$missing" -eq 0 ]; then
  echo "All USB PS Moves already have BlueZ HID SDP pairing records."
  exit 0
fi

exec "$psmove_bin" pair
