# Development Setup

## Tooling

- Rust nightly
- On Windows, prefer the `nightly-x86_64-pc-windows-gnu` toolchain
- QEMU with UEFI firmware support
- LLVM tools
- network access for the first Limine bundle download

## Recommended Reading Before Running Anything

1. [README.md](../../README.md)
2. [docs/architecture/overview.md](../architecture/overview.md)
3. [CONTRIBUTING.md](../../CONTRIBUTING.md)

## Environment Goals

The local environment should support:

- workspace formatting
- workspace type checking
- QEMU execution
- serial-log capture during headless boot checks
- optional debugger attachment for low-level investigation

## Typical Setup Checklist

1. Install Rust nightly and the required components.
2. Install QEMU.
3. Install QEMU with `edk2-x86_64` firmware support.
4. Confirm the workspace is readable by Cargo.
5. Run the baseline validation commands below.

## Local Validation

```bash
cargo run -p xtask -- test
```

## Notes

- `cargo run -p xtask -- run` downloads the pinned Limine v12.3.2 bundle into `dist/limine-v12.3.2/` on first use.
- `cargo run -p xtask -- boot-check` is the preferred runtime validation command for testers and CI.
- On Windows with the GNU toolchain, prefer cloning into an ASCII-only path without spaces if the linker reports object-file lookup failures.
- If QEMU is not installed, build validation may still work but runtime milestones cannot be demonstrated.
