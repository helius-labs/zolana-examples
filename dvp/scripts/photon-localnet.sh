#!/usr/bin/env bash
# Keep temporary SQLite migrations on one connection.
set -euo pipefail
exec "${DVP_PHOTON_BIN:?}" --max-db-conn 1 "$@"
