#!/usr/bin/env bash
# RPM dependency generator for cargo-generate-rpm (`--auto-req <this script>`).
#
# Reads packaged file paths on stdin and prints one RPM requirement per line,
# using only the *direct* dependencies of each ELF file (DT_NEEDED and symbol
# version references), like rpm's own elfdeps. cargo-generate-rpm's builtin
# mode uses `ldd`, which also lists transitive libraries with build-host
# sonames (e.g. libicuuc.so.70) that other distributions don't provide.
set -euo pipefail

while IFS= read -r file; do
  [[ -f $file ]] || continue
  [[ $(head -c 4 "$file" | od -An -c | tr -d ' ') == 177ELF ]] || continue
  marker=""
  if [[ $(od -An -t u1 -j 4 -N 1 "$file" | tr -d ' ') == 2 ]]; then marker="(64bit)"; fi
  objdump -p "$file" | awk -v m="$marker" '
    $1 == "NEEDED" { print $2 "()" m }
    /^Version References:/ { refs = 1; next }
    refs && /^ *required from / { lib = $3; sub(/:$/, "", lib); next }
    refs && lib != "" && NF == 4 && $4 !~ /^\(/ { print lib "(" $4 ")" m }
    refs && /^$/ { refs = 0 }
  '
done | sort -u
