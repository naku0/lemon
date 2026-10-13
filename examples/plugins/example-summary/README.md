# Example summary plugin

Build with `cargo build --release --manifest-path examples/plugins/example-summary/Cargo.toml`.
The executable reads an existing AnalysisBundle through `lemon-plugin-sdk` and writes a compact,
technical JSON summary. It does not recalculate DSP or modify its input.
