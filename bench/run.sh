#!/usr/bin/env bash
# Benchmark hgr against zip / 7zz / rar on a local mixed corpus.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HGR="$ROOT/target/release/hgr"
WORK="/private/tmp/hgr_bench"
CORPUS="$WORK/corpus"
ARCHIVES="$WORK/archives"
EXTRACTED="$WORK/extracted"
RUNS=3

if [[ ! -x "$HGR" ]]; then
  echo "building release binary..."
  ( cd "$ROOT" && cargo build --release >/dev/null )
fi

echo "==> preparing corpus at $CORPUS"
rm -rf "$WORK"
mkdir -p "$CORPUS/text" "$CORPUS/code" "$ARCHIVES" "$EXTRACTED"

# 1) Highly compressible text: concatenated man pages, looped to ~12 MB.
TEXT_TARGET=12000000
TEXT_FILE="$CORPUS/text/manpages.txt"
: > "$TEXT_FILE"
for m in /usr/share/man/man1/*.1 /usr/share/man/man1/*.1.gz; do
  [[ -f "$m" ]] || continue
  if [[ "$m" == *.gz ]]; then
    gunzip -c "$m" 2>/dev/null >> "$TEXT_FILE" || true
  else
    cat "$m" >> "$TEXT_FILE"
  fi
done
# Pad up to target by repeating self until we hit the size.
while (( $(wc -c < "$TEXT_FILE") < TEXT_TARGET )); do
  cat "$TEXT_FILE" >> "$TEXT_FILE.tmp"
  cat "$TEXT_FILE.tmp" >> "$TEXT_FILE"
  rm -f "$TEXT_FILE.tmp"
done
# Trim to target.
TEXT_NOW=$(wc -c < "$TEXT_FILE")
if (( TEXT_NOW > TEXT_TARGET )); then
  dd if="$TEXT_FILE" of="$TEXT_FILE.trim" bs=1 count=$TEXT_TARGET 2>/dev/null
  mv "$TEXT_FILE.trim" "$TEXT_FILE"
fi

# 2) Mixed code: copy the workspace crates as a code corpus.
cp -R "$ROOT/crates" "$CORPUS/code/"

# 3) Binary: copy the hgr release binary itself a few times for ~10 MB total.
HGR_SIZE=$(wc -c < "$HGR")
BIN_COPIES=$(( 10000000 / HGR_SIZE + 1 ))
for i in $(seq 1 "$BIN_COPIES"); do
  cp "$HGR" "$CORPUS/bin_$i"
done

CORPUS_BYTES=$(find "$CORPUS" -type f -exec cat {} + | wc -c | tr -d ' ')
CORPUS_FILES=$(find "$CORPUS" -type f | wc -l | tr -d ' ')
echo "    corpus: $CORPUS_BYTES bytes across $CORPUS_FILES files"
echo

# Timing: best of N runs, wall-clock seconds via /usr/bin/time -p.
best_of() {
  local label="$1"; shift
  local best=999999
  for _ in $(seq 1 "$RUNS"); do
    # /usr/bin/time -p prints "real X.YY" on stderr.
    local t
    t=$( { /usr/bin/time -p "$@" >/dev/null; } 2>&1 | awk '/^real/ {print $2}' )
    awk -v b="$best" -v t="$t" 'BEGIN{ exit !(t+0 < b+0) }' && best=$t
  done
  printf '%s' "$best"
}

declare -a SCENARIOS=(
  # tool|level_label|threads_label|hgr_extra_args (only used for hgr)
  "zip|default|-   |"
  "zip|9      |-   |"
  "7zz|5      |auto|"
  "7zz|9      |auto|"
  "rar|3      |auto|"
  "rar|5      |auto|"
  "hgr|3      |1   |--threads 0"
  "hgr|3      |auto|"
  "hgr|9      |auto|"
  "hgr|9L     |auto|--long"
  "hgr|19     |auto|"
  "hgr|19L    |auto|--long"
  "hgr|9S     |auto|--solid"
  "hgr|9SL    |auto|--solid --long"
  "hgr|19SL   |auto|--solid --long"
)

archive_id() { # tool, level, threads → unique slug for filename
  echo "$1-$2-$3"
}

# Compression pass.
echo "==> compression (best of $RUNS)"
printf '  %-6s %-9s %-7s %12s %8s %10s\n' "tool" "level" "threads" "size_bytes" "vs_input" "best_s"
for sc in "${SCENARIOS[@]}"; do
  IFS='|' read -r tool level threads extra <<< "$sc"
  level="${level// /}"; threads="${threads// /}"
  slug=$(archive_id "$tool" "$level" "$threads")
  out="$ARCHIVES/$slug.${tool}"
  rm -f "$out"
  case "$tool" in
    zip)
      if [[ "$level" == "9" ]]; then zcmd="zip -q9r"; else zcmd="zip -qr"; fi
      cmdline="rm -f \"$out\"; $zcmd \"$out\" \"$CORPUS\""
      ;;
    7zz)
      cmdline="rm -f \"$out\"; 7zz a -bso0 -bsp0 -mx=$level \"$out\" \"$CORPUS\""
      ;;
    rar)
      # rar is interactive on overwrite; -y answers yes. -m<N> sets level.
      cmdline="rm -f \"$out\"; rar a -idq -m$level \"$out\" \"$CORPUS\""
      ;;
    hgr)
      # Strip any letter suffix markers (L = --long, S = --solid) to
      # recover the numeric level passed to --level.
      numeric_level=$(echo "$level" | tr -d 'A-Za-z')
      cmdline="rm -f \"$out\"; \"$HGR\" c \"$out\" --level $numeric_level $extra \"$CORPUS\""
      ;;
  esac
  best=$(best_of "$slug" bash -c "$cmdline")
  if [[ ! -s "$out" ]]; then
    echo "ERROR: $tool produced empty/missing $out" >&2
    echo "       cmdline was: $cmdline" >&2
    exit 1
  fi
  size=$(wc -c < "$out" | tr -d ' ')
  pct=$(awk -v s="$size" -v c="$CORPUS_BYTES" 'BEGIN{ printf "%.1f%%", 100*s/c }')
  printf '  %-6s %-9s %-7s %12s %8s %10s\n' "$tool" "$level" "$threads" "$size" "$pct" "$best"
done

echo
echo "==> extraction (best of $RUNS)"
printf '  %-6s %-9s %-7s %10s\n' "tool" "level" "threads" "best_s"
for sc in "${SCENARIOS[@]}"; do
  IFS='|' read -r tool level threads _ <<< "$sc"
  level="${level// /}"; threads="${threads// /}"
  slug=$(archive_id "$tool" "$level" "$threads")
  src="$ARCHIVES/$slug.${tool}"
  dest="$EXTRACTED/$slug"
  case "$tool" in
    zip) cmdline="rm -rf \"$dest\"; mkdir -p \"$dest\"; unzip -q \"$src\" -d \"$dest\"" ;;
    7zz) cmdline="rm -rf \"$dest\"; mkdir -p \"$dest\"; 7zz x -bso0 -bsp0 -y \"$src\" -o\"$dest\"" ;;
    rar) cmdline="rm -rf \"$dest\"; mkdir -p \"$dest\"; unrar x -idq -y \"$src\" \"$dest\"/" ;;
    hgr) cmdline="rm -rf \"$dest\"; mkdir -p \"$dest\"; \"$HGR\" x \"$src\" -o \"$dest\"" ;;
  esac
  best=$(best_of "x-$slug" bash -c "$cmdline")
  printf '  %-6s %-9s %-7s %10s\n' "$tool" "$level" "$threads" "$best"
done

echo
echo "==> done. corpus=$CORPUS_BYTES bytes, runs=$RUNS"
