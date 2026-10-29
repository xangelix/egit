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

## Fonts

egit's interface is set in [Geist](https://github.com/vercel/geist-font), shipped in
`crates/egit/fonts/` under the SIL Open Font License 1.1 (`OFL-Geist.txt`).

## License

egit is licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in egit by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
