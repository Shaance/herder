#!/bin/sh
# Keep one executable inode while each fixture owns its editable script body.
IFS= read -r interpreter < "$0.body"
case "$interpreter" in
    '#!/bin/sh') . "$0.body" ;;
    '#!/usr/bin/python3') exec /usr/bin/python3 "$0.body" "$@" ;;
    '#!/usr/bin/env bash') exec /bin/bash "$0.body" "$@" ;;
    *) echo "unsupported fixture interpreter: $interpreter" >&2; exit 1 ;;
esac
