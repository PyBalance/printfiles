# Contributing to printfiles

Thanks for your interest in improving `printfiles`! Please follow these
guidelines to keep the project healthy and easy to maintain.

## Prerequisites

- Rust toolchain 1.70 or newer (`rustup override set 1.70.0` is a convenient
  way to pin the toolchain locally).
- `cargo` with `fmt` and `clippy` components installed
  (`rustup component add rustfmt clippy`).

## Development Workflow

1. Create a feature branch off `main`.
2. Make your changes.
3. Run the mandatory checks locally:
   ```bash
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```
4. Ensure any new functionality is covered by unit tests and, when appropriate,
   integration tests under `tests/` using `assert_cmd`/`assert_fs`.
5. Update documentation (`README.md`, `TASK.md`, CHANGELOG) when behaviour or
   usage changes.
6. Commit with clear, conventional messages and open a pull request describing
   the motivation and testing performed.

## Reporting Issues

Please include:

- Steps to reproduce
- Expected behaviour
- Actual behaviour
- Platform details (OS, Rust version)

## Code Style

- Let `cargo fmt` dictate formatting.
- Prefer small helper functions over large blocks of inline logic when control
  flow becomes complex (e.g., new reader strategies or filtering rules).
- Add concise comments ahead of non-obvious sections to aid future readers.

Thank you for contributing!
