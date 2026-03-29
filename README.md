# build-pulse

A research-focused Jenkins CI/CD static build analyzer.

<img width="1363" height="1597" alt="image" src="https://github.com/user-attachments/assets/8ac08567-4aca-4386-b5d6-873ae268072b" />

## Quick Start

### Requirements

- `openssl` (bundled by default)
- `pkg-config`
- `python3`
- `rust`
- `sqlite` (bundled by default)
- `uv`
- (optional) `nix`

### Installation

- Install requirements or run `nix develop`
- `cargo build --release`
- `./target/release/build-pulse -o report.html`

Check `config.toml` for runtime configuration.

## Additional Documentation

- Run `cargo doc` for crate
- Supporting writeups are in `docs/`
