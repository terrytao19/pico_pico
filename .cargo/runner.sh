#!/usr/bin/env bash
set -euo pipefail

ELF="$1"
UF2="${ELF}.uf2"

elf2uf2-rs "$ELF" "$UF2"
picotool load -u -v -x "$UF2"