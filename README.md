# arxiv-tex-extract

Experimental Rust utility for extracting plain text from arXiv LaTeX source archives. Processes nested `.tar` / `.tar.gz` archives containing LaTeX papers, converts them to text, and writes results to Parquet, JSONL, or individual text files.


## Installation


```bash
git clone <repo-url>
cd latex_extract
cargo build --release
```

The binary will be at `target/release/latex-extract`.

On non-MSVC targets (Linux, macOS), [jemalloc](https://github.com/tikv/jemallocator) is used as the global allocator for improved performance and memory statistics.

## Usage

### Single file

Process one archive and print JSON to stdout:

```bash
latex-extract -f paper.tar.gz
```

### Batch mode (Parquet)

Process a directory of outer `.tar` files, writing zstd-compressed Parquet shards:

```bash
latex-extract -d /data/arxiv_tars -o /data/results
```

### Batch mode (JSONL)

```bash
latex-extract -d /data/arxiv_tars -o /data/results --output-format jsonl
```

### Text files

Write one `.txt` file per paper:

```bash
latex-extract -d /data/arxiv_tars -o /data/texts --text-files
```

### Resume an interrupted run

```bash
latex-extract -d /data/arxiv_tars -o /data/results --resume
```

### Full example with all options

```bash
latex-extract \
  -d /data/arxiv_tars \
  -o /data/results \
  -j 16 \
  -t 60 \
  --output-format parquet \
  --max-shard-rows 5000 \
  --max-shard-bytes 128000000 \
  --papers-per-shard 256 \
  --resume \
  --metrics
```

`--papers-per-shard 256` bounds the blast radius of a process-wide SIGKILL
from OOMs to the most recent shard rather than the whole
tar. See [Jetsam / OOM resilience](#jetsam--oom-resilience) below.

## CLI Reference

| Flag | Description | Default |
|------|-------------|---------|
| `-d, --input-dir <PATH>` | Directory of outer `.tar` files (batch mode) | — |
| `-f, --input-file <PATH>` | Single archive file (`.tar.gz`, `.gz`, or `.tex`) | — |
| `-o, --output-dir <PATH>` | Output directory (required for batch mode) | — |
| `-j, --threads <N>` | Worker threads | number of CPUs |
| `-t, --timeout-secs <N>` | Per-document extraction timeout | `45` |
| `--max-tex-bytes <N>` | Max combined `.tex` content before skipping | `20000000` (20 MB) |
| `--text-files` | Write one `.txt` per paper instead of structured output | off |
| `--output-format <FMT>` | `parquet` or `jsonl` | `parquet` |
| `--max-shard-rows <N>` | Max rows per Parquet shard | `10000` |
| `--max-shard-bytes <N>` | Max uncompressed bytes per Parquet shard | `256000000` (256 MB) |
| `--papers-per-shard <N>` | Rotate to a new shard every N papers; activates per-paper checkpointing for jetsam-resilient `--resume` | unset |
| `--resume` | Skip work already recorded in `checkpoint.log` (tar-level always; per-paper when `--papers-per-shard` is set) | off |
| `--metrics` | Emit `{stem}_metrics.json` sidecar per tar | off |

Either `--input-dir` or `--input-file` must be provided. Batch mode (`--input-dir`) requires `--output-dir`.

## Output Schema

Each extracted paper produces one record with these fields:

| Field | Type | Description |
|-------|------|-------------|
| `arxiv_id` | string | Paper identifier derived from the archive filename |
| `source_tar` | string? | Name of the outer tar file (batch mode only) |
| `status` | string | `ok`, `error`, `timeout`, `skipped`, or `empty` |
| `num_tex_files` | uint? | Number of `.tex` files found in the archive |
| `text_length` | uint? | Character count of extracted text |
| `text` | string? | The extracted plain text |
| `error` | string? | Error message if status is not `ok` |
| `stage_timings_us` | string? | JSON map of pipeline stage name to microseconds |
| `total_time_us` | uint64? | Total pipeline wall-clock time in microseconds |
| `peak_memory_bytes` | uint64? | Best-effort peak memory estimate |

### Parquet sharding

Parquet output is automatically split into shards when any of the configured
limits is reached: `--max-shard-rows`, `--max-shard-bytes`, or
`--papers-per-shard` (first-fired wins). Shard files are named
`{stem}.parquet`, `{stem}_001.parquet`, `{stem}_002.parquet`, etc. All
shards use zstd compression (level 3) and are written atomically via
temp-file-then-rename. On `--resume`, the writer scans existing shards
for a tar's stem and starts new shards after the highest found index so
prior-run output is never overwritten.

JSONL output follows the same rotation rules and filename convention
(`{stem}.jsonl`, `{stem}_001.jsonl`, …) when `--papers-per-shard` is set;
without the flag, JSONL writes a single file per tar as before.

### Error categorization

Extraction outcomes are classified into fine-grained categories rather than a single "error" bucket:

| Status | Meaning |
|--------|---------|
| `ok` | Successful extraction |
| `empty` | No `.tex` files found, or extraction produced no text |
| `skipped` | Combined `.tex` content exceeds size limit (default 20 MB) |
| `skipped_resume` | Paper was already recorded as durable by a prior run — extraction bypassed |
| `timeout` | Per-document extraction timeout fired |
| `panic` | Extraction pipeline panicked (caught by `catch_unwind`) |
| `crash` | Extraction thread disconnected unexpectedly |
| `archive_error` | Tar entry, archive load, or decompression failure |
| `io_error` | Failed to write output (Parquet, JSONL, or text file) |

Each error is logged with a structured `category` field for easy filtering:

```bash
# Show only panics
RUST_LOG=info latex-extract ... 2>&1 | grep 'category=panic'

# Show only archive failures
RUST_LOG=info latex-extract ... 2>&1 | grep 'category=archive'
```

The summary log shows a breakdown with sample IDs for non-zero categories:

```
Done: 50 tars, 125000 papers (123500 ok)
  200 timeouts (e.g. 2401.00100, 2401.00200, 2401.00300, 2401.00400, 2401.00500)
  400 skipped (e.g. 2401.10000, 2401.10001)
  2300 skipped (resume) (e.g. 2401.20000, 2401.20001)
  150 panics (e.g. 2401.00001, 2401.00523, 2401.01234)
  130 archive errors (e.g. hep-ph/0001001, hep-ph/0001002)
  100 empty
  20 crashes (e.g. 2401.99999)
  12 I/O write errors
```

### Metrics sidecar

When `--metrics` is enabled, each processed tar produces a JSON file like:

```json
{
  "tar_name": "arXiv_src_0712.tar",
  "total_papers": 5000,
  "ok": 4850,
  "empty": 60,
  "skipped": 40,
  "skipped_resume": 0,
  "timeouts": 20,
  "panics": 15,
  "crashes": 2,
  "archive_errors": 13,
  "io_errors": 0,
  "processing_time_ms": 45000
}
```

## Extraction Pipeline

LaTeX-to-text conversion is a multi-stage pipeline. Each stage is timed independently and reported in the output.

| # | Stage | Description |
|---|-------|-------------|
| 1 | Input resolution | Parse `\input{}` / `\include{}` directives, identify the main file, order sources |
| 2 | Macro collection | Extract `\newcommand`, `\renewcommand`, and `\def` definitions |
| 3 | Theorem collection | Collect `\newtheorem` definitions for custom environments |
| 4 | Comment removal | Strip `%`-comments, handling escaped `\%` and line joining |
| 5 | Macro expansion | Replace macro calls with definitions (multi-pass, handles nesting) |
| 6 | Structure conversion | Sections to markdown headers, abstract/acknowledgements extraction, theorem environments, author/affiliation parsing (REVTeX, Elsevier, Springer styles) |
| 7 | Reference conversion | `\ref` / `\cite` to readable text, bibliography mapping, journal abbreviations |
| 8 | Formatting conversion | `\emph{}` to `*...*`, `\textbf{}` to `...`, unwrap text commands |
| 9 | Environment conversion | Lists, figures, tables, proofs, equations, multi-column layouts |
| 10 | Pre-diacritic cleanup | Remove dimension commands before diacritic pass |
| 11 | Diacritic conversion | LaTeX accents (`\'{e}`, `\"o`) to Unicode characters |
| 12 | Symbol conversion | Aho-Corasick multi-pattern replacement of math/special symbols |
| 13 | Final cleanup | Strip remaining LaTeX commands, normalize whitespace |

## Safety Limits

| Limit | Value | Rationale |
|-------|-------|-----------|
| Max combined `.tex` size | 20 MB (default, configurable via `--max-tex-bytes`) | Prevents regex explosion across pipeline passes |
| Max decompressed entry | 100 MB | Guards against zip bombs |
| Per-document timeout | 45 s (default) | Stuck documents are killed; worker threads are not blocked |

Documents exceeding the `.tex` size limit are reported with status `skipped`. Timed-out documents are reported with status `timeout`.

## Checkpoint and Resume

Batch mode writes `checkpoint.log` in the output directory (fsync'd after each
write). It carries two kinds of entries, parsed by field count so both coexist:

- `tar_name` (one field) — the whole tar completed successfully.
- `tar_name\tarxiv_id` (two fields, tab-separated) — that specific paper
  is already in a readable shard on disk. Only emitted when
  `--papers-per-shard` is set.

`--resume` uses both: it skips whole tars found in the first form, and inside
remaining tars it skips papers found in the second form (classified as
`skipped_resume`) before any extraction work runs. Stale `.tmp` shards from
a prior-run kill are deleted at tar start — by invariant (below) they
only hold non-checkpointed rows, so discarding them is safe.

## Jetsam / OOM resilience

Long batch runs on large corpora can hit process-wide SIGKILL from OOM errors. Without `--papers-per-shard`, a kill in the
middle of a tar loses every paper in the in-flight tar's output (Parquet
files only become readable once the footer is written at shard close).

With `--papers-per-shard N`, two things change:

1. The writer closes and renames a shard every N papers
   (in addition to the existing `--max-shard-rows` / `--max-shard-bytes`
   triggers). Each completed shard is a standalone file with its own
   footer, meaning its readable immediately, independent of any later crash.
2. When a shard closes successfully
   (`.tmp` → final rename), every paper ID it contained is appended to
   `checkpoint.log` and fsynced. On rerun, those papers are skipped.

The invariant this preserves:

> Every `arxiv_id` in `checkpoint.log` is in a readable shard footer on
> disk; nothing in a `.tmp` file is ever in the checkpoint.

A mid-tar kill loses at most the in-flight (not-yet-rotated) shard's
papers, so the rerun re-extracts them and writes them into new shards that
don't overwrite prior output.

### Usage

```bash
# First invocation — run until done or until the OS kills the process.
latex-extract \
  -d /data/arxiv_tars \
  -o /data/results \
  -j 2 \
  --output-format parquet \
  --papers-per-shard 256 \
  --resume \
  --metrics

# On any interruption (jetsam, Ctrl-C, power loss), rerun with the same
# command. Durable papers are skipped; only the in-flight shard's work
# (≤ 256 papers) is redone.
```

At 256 papers/shard, a large-corpus run produces many small shards. An
offline coalescing pass (e.g. `parquet merge` or `duckdb COPY`) may be useful for downstream consumers.

## Development

### Run tests

```bash
cargo test
```

### Run benchmarks

```bash
cargo bench
```

Benchmarks use [criterion](https://github.com/bheisler/criterion.rs) and cover full extraction on small (31 KB), medium (66 KB), and large (134 KB) LaTeX fixtures, plus per-stage timing.

### Project structure

```
src/
├── main.rs            CLI, batch orchestration, timeout isolation
├── pipeline.rs        Pipeline stage sequencing and timing
├── archive.rs         Nested tar/gz archive extraction
├── input_resolve.rs   \input/\include resolution and file ordering
├── preamble.rs        Preamble parsing, author/title extraction
├── macros.rs          Macro collection and expansion
├── comments.rs        Comment stripping
├── structure.rs       Section/heading conversion, document structure
├── references.rs      Citation and reference handling
├── formatting.rs      Text formatting conversion
├── environments.rs    List/table/figure/equation processing
├── diacritics.rs      LaTeX accent to Unicode conversion
├── symbols.rs         Aho-Corasick symbol replacement
├── braces.rs          Brace matching utilities
├── cleanup.rs         Final command removal and whitespace normalization
├── output.rs          Parquet shard writer
├── checkpoint.rs      Resume checkpoint management
├── metrics.rs         Per-tar metrics output
├── result.rs          ExtractionResult schema
└── timing.rs          Per-stage wall-clock timing
tests/
└── integration.rs     End-to-end parquet and checkpoint tests
benches/
├── throughput.rs      Criterion benchmarks
└── fixtures/          Sample LaTeX files
```
