#!/usr/bin/env sh
set -eu
root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
out="${TMPDIR:-/tmp}/arcade-protocol-test"
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror \
  -I"$root/protocol/include" \
  "$root/protocol/src/protocol.cpp" \
  "$root/protocol/test/test_protocol.cpp" \
  -o "$out"
"$out"
