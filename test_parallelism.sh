#!/usr/bin/env bash
set -euo pipefail

# Test script for paper-level parallelism changes (commit bd9978a)
# Bundles sample .tar.gz files into outer .tar files, then runs the binary
# with various configurations to verify correctness and measure performance.

BINARY="./target/release/latex-extract"
SAMPLE_DIR="/Users/adambuttrick/Downloads/latex_statements/latex_samples"
TEST_BASE="/tmp/parallelism_test_$$"
LARGE_DIR="$TEST_BASE/inputs/large"
SMALL_DIR="$TEST_BASE/inputs/small"
ALL_DIR="$TEST_BASE/inputs/all"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass_count=0
fail_count=0

pass() { echo -e "  ${GREEN}PASS${NC}: $1"; pass_count=$((pass_count + 1)); }
fail() { echo -e "  ${RED}FAIL${NC}: $1"; fail_count=$((fail_count + 1)); }
info() { echo -e "${YELLOW}==>${NC} $1"; }

cleanup() {
    info "Cleaning up $TEST_BASE"
    rm -rf "$TEST_BASE"
}
trap cleanup EXIT

# ── Step 1: Verify prerequisites ─────────────────────────────────────────────

if [[ ! -x "$BINARY" ]]; then
    echo "Binary not found at $BINARY — run 'cargo build --release' first"
    exit 1
fi

if [[ ! -d "$SAMPLE_DIR" ]]; then
    echo "Sample data not found at $SAMPLE_DIR"
    exit 1
fi

# ── Step 2: Create mock outer tars ───────────────────────────────────────────

info "Creating test directories"
mkdir -p "$LARGE_DIR" "$SMALL_DIR" "$ALL_DIR"

# Write sorted file list to a temp file (avoids bash array issues with .tar.gz names)
FILELIST="$TEST_BASE/filelist.txt"
ls "$SAMPLE_DIR"/*.tar.gz | sort | xargs -I{} basename {} > "$FILELIST"
TOTAL=$(wc -l < "$FILELIST" | tr -d ' ')
info "Found $TOTAL sample .tar.gz files"

# Helper: create an outer tar from lines of a file list
# Usage: make_outer_tar <output.tar> <list_file>
make_outer_tar() {
    local outpath="$1"
    local listfile="$2"
    local count
    count=$(wc -l < "$listfile" | tr -d ' ')
    tar cf "$outpath" -C "$SAMPLE_DIR" -T "$listfile"
    echo "  Created $(basename "$outpath") with $count papers"
}

# Large tars: ~500 papers each
info "Building large outer tars (~500 papers each)"
split -l 500 -d -a 3 "$FILELIST" "$TEST_BASE/large_chunk_"
batch_num=1
for chunk_file in "$TEST_BASE"/large_chunk_*; do
    outfile="$LARGE_DIR/batch_$(printf '%03d' $batch_num).tar"
    make_outer_tar "$outfile" "$chunk_file"
    ln -sf "$outfile" "$ALL_DIR/"
    ((batch_num++))
done

# Small tars: ~50 papers each (use first 200 samples)
info "Building small outer tars (~50 papers each)"
head -200 "$FILELIST" > "$TEST_BASE/small_list.txt"
split -l 50 -d -a 3 "$TEST_BASE/small_list.txt" "$TEST_BASE/small_chunk_"
batch_num=1
for chunk_file in "$TEST_BASE"/small_chunk_*; do
    outfile="$SMALL_DIR/batch_small_$(printf '%03d' $batch_num).tar"
    make_outer_tar "$outfile" "$chunk_file"
    ln -sf "$outfile" "$ALL_DIR/"
    ((batch_num++))
done

# Tiny tar: 5 papers
info "Building tiny outer tar (5 papers)"
head -5 "$FILELIST" > "$TEST_BASE/tiny_list.txt"
make_outer_tar "$SMALL_DIR/batch_tiny.tar" "$TEST_BASE/tiny_list.txt"
ln -sf "$SMALL_DIR/batch_tiny.tar" "$ALL_DIR/"

# Edge case: empty tar
info "Building edge-case tars"
tar cf "$SMALL_DIR/empty.tar" --files-from /dev/null
echo "  Created empty.tar (0 papers)"

# Edge case: single-paper tar
head -1 "$FILELIST" > "$TEST_BASE/single_list.txt"
make_outer_tar "$SMALL_DIR/single.tar" "$TEST_BASE/single_list.txt"

# ── Step 3: Functional correctness tests ─────────────────────────────────────

info "Test 3a: Parquet output (small tars)"
OUT_PARQUET="$TEST_BASE/output_parquet"
mkdir -p "$OUT_PARQUET"
"$BINARY" -d "$SMALL_DIR" -o "$OUT_PARQUET" --output-format parquet --resume --metrics 2>&1 | tail -5

# Check parquet files exist per tar (skip empty tar)
for tarfile in "$SMALL_DIR"/*.tar; do
    stem=$(basename "$tarfile" .tar)
    if [[ "$stem" == "empty" ]]; then continue; fi
    pq="$OUT_PARQUET/${stem}.parquet"
    if [[ -f "$pq" ]]; then
        pass "parquet file exists: ${stem}.parquet"
    else
        fail "missing parquet file: ${stem}.parquet"
    fi
done

# Check checkpoint
if [[ -f "$OUT_PARQUET/checkpoint.log" ]]; then
    checkpoint_lines=$(wc -l < "$OUT_PARQUET/checkpoint.log" | tr -d ' ')
    pass "checkpoint.log exists ($checkpoint_lines entries)"
else
    fail "checkpoint.log missing"
fi

# Check metrics
metrics_count=$(find "$OUT_PARQUET" -name "*_metrics.json" | wc -l | tr -d ' ')
if (( metrics_count > 0 )); then
    pass "metrics files exist ($metrics_count files)"
    # Validate JSON
    first_metrics=$(find "$OUT_PARQUET" -name "*_metrics.json" | head -1)
    if python3 -c "import json; json.load(open('$first_metrics'))" 2>/dev/null; then
        pass "metrics JSON is valid"
    else
        fail "metrics JSON is invalid"
    fi
else
    fail "no metrics files found"
fi

echo ""
info "Test 3b: JSONL output (small tars)"
OUT_JSONL="$TEST_BASE/output_jsonl"
mkdir -p "$OUT_JSONL"
"$BINARY" -d "$SMALL_DIR" -o "$OUT_JSONL" --output-format jsonl --resume 2>&1 | tail -5

for tarfile in "$SMALL_DIR"/*.tar; do
    stem=$(basename "$tarfile" .tar)
    if [[ "$stem" == "empty" ]]; then continue; fi
    jl="$OUT_JSONL/${stem}.jsonl"
    if [[ -f "$jl" ]]; then
        line_count=$(wc -l < "$jl" | tr -d ' ')
        pass "JSONL file exists: ${stem}.jsonl ($line_count lines)"
        # Validate first line is valid JSON
        if head -1 "$jl" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
            pass "JSONL first line is valid JSON: ${stem}.jsonl"
        else
            fail "JSONL invalid JSON in: ${stem}.jsonl"
        fi
    else
        fail "missing JSONL file: ${stem}.jsonl"
    fi
done

echo ""
info "Test 3c: Text file output (small tars)"
OUT_TEXT="$TEST_BASE/output_text"
mkdir -p "$OUT_TEXT"
"$BINARY" -d "$SMALL_DIR" -o "$OUT_TEXT" --text-files 2>&1 | tail -5

txt_count=$(find "$OUT_TEXT" -name "*.txt" | wc -l | tr -d ' ')
if (( txt_count > 0 )); then
    pass "text files created ($txt_count files)"
    # Spot-check: first file is non-empty
    first_txt=$(find "$OUT_TEXT" -name "*.txt" | head -1)
    first_size=$(wc -c < "$first_txt" | tr -d ' ')
    if (( first_size > 0 )); then
        pass "first text file is non-empty ($first_size bytes)"
    else
        fail "first text file is empty"
    fi
else
    fail "no text files created"
fi

# ── Step 4: Concurrency & thread variations ──────────────────────────────────

echo ""
info "Test 4: Concurrency variations (large tars)"

reference_rows=""
all_match=true
timing_summary=""

for ct in 1 2 4 8; do
    outdir="$TEST_BASE/output_ct${ct}"
    mkdir -p "$outdir"
    info "  Running --concurrent-tars $ct"

    start_time=$(date +%s)
    "$BINARY" -d "$LARGE_DIR" -o "$outdir" --output-format parquet --resume --concurrent-tars "$ct" 2>&1 | tail -3
    end_time=$(date +%s)
    elapsed=$(( end_time - start_time ))

    # Count total parquet rows across all files
    total_rows=0
    for pq in "$outdir"/*.parquet; do
        if [[ -f "$pq" ]]; then
            rows=$(python3 -c "
import pyarrow.parquet as pq
t = pq.read_table('$pq')
print(len(t))
" 2>/dev/null || echo "0")
            total_rows=$((total_rows + rows))
        fi
    done
    echo "    concurrent-tars=$ct: $total_rows rows, time=${elapsed}s"
    timing_summary="${timing_summary}  --concurrent-tars $ct: ${elapsed}s\n"

    if [[ -z "$reference_rows" ]]; then
        reference_rows=$total_rows
    elif [[ "$total_rows" != "$reference_rows" ]]; then
        all_match=false
        fail "row count mismatch: ct=1 has $reference_rows rows, ct=$ct has $total_rows rows"
    fi

    # Clean up parquet output between runs to save disk
    rm -rf "$outdir"
done

if $all_match; then
    pass "all concurrency levels produce identical row counts ($reference_rows rows)"
fi

echo ""
info "Timing summary:"
echo -e "$timing_summary"

# ── Step 5: Checkpoint/resume test ───────────────────────────────────────────

echo ""
info "Test 5: Checkpoint/resume"

OUT_RESUME="$TEST_BASE/output_resume"
mkdir -p "$OUT_RESUME"
"$BINARY" -d "$SMALL_DIR" -o "$OUT_RESUME" --output-format parquet --resume 2>&1 | tail -3

if [[ -f "$OUT_RESUME/checkpoint.log" ]]; then
    pass "checkpoint.log created after initial run"
else
    fail "checkpoint.log not created"
fi

# Re-run — should skip everything
output=$("$BINARY" -d "$SMALL_DIR" -o "$OUT_RESUME" --output-format parquet --resume 2>&1)
if echo "$output" | grep -q "All tars already processed"; then
    pass "resume correctly skips completed tars"
else
    fail "resume did not skip completed tars"
fi

# Remove one entry and reprocess
if [[ -f "$OUT_RESUME/checkpoint.log" ]]; then
    # Remove last line from checkpoint
    removed_tar=$(tail -1 "$OUT_RESUME/checkpoint.log")
    removed_stem="${removed_tar%.tar}"
    sed -i '' '$ d' "$OUT_RESUME/checkpoint.log"
    rm -f "$OUT_RESUME/${removed_stem}.parquet"

    "$BINARY" -d "$SMALL_DIR" -o "$OUT_RESUME" --output-format parquet --resume 2>&1 | tail -3

    if [[ -f "$OUT_RESUME/${removed_stem}.parquet" ]]; then
        pass "resume reprocessed removed tar: $removed_tar"
    else
        fail "resume did not reprocess removed tar: $removed_tar"
    fi
fi

# ── Step 6: Edge case tests ──────────────────────────────────────────────────

echo ""
info "Test 6: Edge cases"

# Empty tar — should not crash
OUT_EDGE="$TEST_BASE/output_edge"
mkdir -p "$OUT_EDGE/empty_test" "$OUT_EDGE/empty_out"
cp "$SMALL_DIR/empty.tar" "$OUT_EDGE/empty_test/"
if "$BINARY" -d "$OUT_EDGE/empty_test" -o "$OUT_EDGE/empty_out" --output-format parquet --resume 2>&1; then
    pass "empty tar does not crash"
else
    fail "empty tar caused a crash"
fi

# Single-paper tar
mkdir -p "$OUT_EDGE/single_test" "$OUT_EDGE/single_out"
cp "$SMALL_DIR/single.tar" "$OUT_EDGE/single_test/"
if "$BINARY" -d "$OUT_EDGE/single_test" -o "$OUT_EDGE/single_out" --output-format parquet --resume 2>&1 | tail -3; then
    if [[ -f "$OUT_EDGE/single_out/single.parquet" ]]; then
        pass "single-paper tar produces output"
    else
        fail "single-paper tar produced no parquet"
    fi
else
    fail "single-paper tar caused a crash"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════"
echo -e "  ${GREEN}Passed${NC}: $pass_count"
echo -e "  ${RED}Failed${NC}: $fail_count"
echo "════════════════════════════════════════════════════"

if (( fail_count > 0 )); then
    exit 1
fi
