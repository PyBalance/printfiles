# printfiles

A fast CLI helper that prints the contents of many files at once, wrapping each
file with `===path===` / `===end of 'path'===` markers to make diffs or code
reviews easier to read.

## Features

- Accepts a mix of comma- and space-separated glob patterns and directories in a
  single invocation.
- Recursively descends into directories (honouring `--ext` filters when
  provided).
- Supports three reader backends: plain text (`text`), macOS `textutil`
  (`textutil`), and hybrid auto-detection (`auto`).
- Emits output in sorted order with stable headers, so results are deterministic.
- Falls back gracefully when `textutil` is unavailable or fails.

## Requirements

- Rust 1.70 or newer.
- macOS users who want Office/RTF/HTML rendering must have the `textutil`
  command available (bundled with macOS). On Linux/Windows the tool will fall
  back to raw text output automatically.

## Installation

```bash
cargo install --path .
```

To build without installing, run:

```bash
cargo build --release
```

The resulting binary is at `target/release/printfiles`.

## Usage

Basic syntax:

```bash
printfiles [OPTIONS] <patterns-or-directories>...
```

Key options:

- `--reader <text|textutil|auto>` (default: `text`)
- `--ext <csv>`: limit files discovered via directory arguments to the listed
  extensions (comma separated, case-insensitive)
- `--relative-from <dir>`: display headers relative to the provided directory;
  paths outside the directory fall back to the current working directory
- `--max-size <bytes>`: skip files larger than the given number of bytes (with a
  notice on stderr and a placeholder in output)
- `--binary <skip|hex|base64|print>`: control how likely-binary files are
  handled (skip, hex dump, Base64, or force text)
- `--sort <name|size|mtime>`: reorder matched files by path, byte size, or
  modified time (ascending)
- `--follow-links[=true|false]`: choose whether directory/glob searches follow
  symbolic links (default: true)
- `--quiet` / `--verbose`: control logging noise on stderr

### Examples

```bash
# Print all Rust sources under src/ and Markdown docs under docs/
printfiles "src/**/*.rs,docs/*.md"

# Mix spaces and commas freely
printfiles src/**/*.rs docs/*.md,tests/*.rs

# Traverse a directory but only include certain extensions
printfiles src --ext rs,md

# Force textutil rendering (macOS only)
printfiles reports/**/*.docx --reader textutil

# Auto-detect rich-text formats while still filtering extensions on directories
printfiles reports docs --ext md,docx --reader auto

# Rebase headers relative to a different root
printfiles src/**/*.rs --relative-from src

# Skip files larger than 1 MiB
printfiles logs/**/*.log --max-size 1048576

# Dump binary files as hex without skipping
printfiles assets/**/*.bin --binary hex

# Sort results by file size instead of name
printfiles logs/**/*.log --sort size

# Silence warnings while still producing content
printfiles logs/**/*.log --max-size 1024 --quiet
```

## Exit Codes

- `0`: All files were read and printed successfully.
- `1`: At least one file failed to read; errors are reported on stderr.
- `2`: No files matched the requested patterns.

## Platform Notes

- **macOS**: `--reader textutil` and `--reader auto` leverage the system
  `textutil` command when it is present. Failures fall back to raw text output
  with a warning.
- **Other platforms**: `textutil` is not available; the tool prints a notice and
  proceeds with raw text reading.
- Symbolic links are followed by default (via `globwalk`'s `follow_links(true)`).

## Development

Run the standard checks before submitting patches:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

The integration tests live under `tests/` and rely on temporary directories, so
no fixtures are required.

## License

MIT
