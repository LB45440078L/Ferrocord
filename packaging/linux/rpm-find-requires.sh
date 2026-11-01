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
  # A version reference line is "<hash> <flags> <index> <name>", e.g.
  # "0x06969197 0x00 02 GLIBC_2.17". The section ends at the first line that is
  # not indented: a blank line, or "private flags = 0x0:" from aarch64 objdump,
  # which must not turn into a bogus "libc.so.6(0x0:)" requirement.
  objdump -p "$file" | awk -v m="$marker" '
    $1 == "NEEDED" { print $2 "()" m }
    /^Version References:/ { refs = 1; lib = ""; next }
    refs && !/^[ \t]/ { refs = 0 }
    refs && /^ *required from / { lib = $3; sub(/:$/, "", lib); next }
    refs && lib != "" && NF == 4 && $1 ~ /^0x[0-9a-fA-F]+$/ && $4 !~ /^\(/ { print lib "(" $4 ")" m }
  '
done | sort -u | awk '
  # Fail the build rather than ship a package no distribution can install.
  !/^[^ ()]+\(([A-Za-z_][A-Za-z0-9_.+-]*)?\)(\(64bit\))?$/ {
    print "rpm-find-requires: malformed requirement: " $0 > "/dev/stderr"; bad = 1
  }
  { print }
  END { exit bad }
'
