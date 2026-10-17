# egit

A native git client in [egui](https://github.com/emilk/egui), built to stay fast on the largest
repositories.

egit drives the `git` installed on your system for every repository operation, so hooks,
signing, credential helpers, and your configuration behave exactly as they do on the command
line. It loads and draws only what is on screen.

It is early work: see [the roadmap](docs/ROADMAP.md).

## Requirements

- Linux (Ubuntu 24.04 LTS or newer, or an equivalent distribution); macOS 11 or newer; or
  Windows 10 or newer. Linux is egit's home; macOS and Windows are built and tested by CI.
- git 2.43.0 or newer (on Windows, Git for Windows)

## Building

```sh
cargo build --release
```

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first use.
