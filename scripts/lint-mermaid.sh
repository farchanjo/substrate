#!/usr/bin/env bash
# lint-mermaid.sh — validate every Mermaid block in every .md under the project
# by feeding each block through mmdc (mermaid-cli). Uses Perl for block
# extraction. Fails on first error unless --keep-going is passed.

set -euo pipefail

KEEP_GOING=0
[[ "${1:-}" == "--keep-going" ]] && KEEP_GOING=1

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v mmdc >/dev/null 2>&1 || { echo "ERROR: mmdc not in PATH" >&2; exit 127; }
command -v perl >/dev/null 2>&1 || { echo "ERROR: perl not in PATH" >&2; exit 127; }

FAILED=0
BLOCKS=0
FILES=0

mapfile -t MD_FILES < <(find "$ROOT" -type f -name '*.md' \
    -not -path '*/target/*' -not -path '*/.git/*' \
    -not -path '*/node_modules/*' -not -path '*/.venv/*' \
    -not -path '*/.spec-cache/*' | sort)

for MD in "${MD_FILES[@]}"; do
    REL="${MD#$ROOT/}"
    SAFE="${REL//\//_}"
    SAFE="${SAFE//./_}"

    # Extract every ```mermaid ... ``` block to a separate .mmd file via perl.
    perl -e '
        my ($md, $tmp, $safe) = @ARGV;
        open my $in, "<", $md or die "open $md: $!";
        local $/; my $text = <$in>; close $in;
        my $i = 0;
        while ($text =~ /^```mermaid[ \t]*\n(.*?)^```[ \t]*$/smg) {
            $i++;
            my $body = $1;
            my $path = sprintf("%s/%s.%03d.mmd", $tmp, $safe, $i);
            open my $fh, ">", $path or die "open $path: $!";
            print $fh $body;
            close $fh;
        }
    ' "$MD" "$TMP" "$SAFE"

    FILE_BLOCKS=0
    while IFS= read -r -d '' BLOCK; do
        BLOCKS=$((BLOCKS+1))
        FILE_BLOCKS=$((FILE_BLOCKS+1))
        if ! OUT=$(mmdc -i "$BLOCK" -o "$TMP/out.svg" -q 2>&1); then
            FAILED=$((FAILED+1))
            BASE="${BLOCK%.mmd}"
            IDX="${BASE##*.}"
            echo "FAIL $REL block #$IDX" >&2
            echo "$OUT" | sed 's/^/      /' >&2
            [[ $KEEP_GOING -eq 0 ]] && exit 1
        fi
    done < <(find "$TMP" -maxdepth 1 -name "${SAFE}.*.mmd" -print0 2>/dev/null)

    if [[ $FILE_BLOCKS -gt 0 ]]; then
        FILES=$((FILES+1))
        printf "ok %s (%d blocks)\n" "$REL" "$FILE_BLOCKS"
    fi
done

echo "---"
echo "Mermaid lint: $BLOCKS blocks across $FILES files; $FAILED failures"
exit "$FAILED"
