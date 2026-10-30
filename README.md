# egit

A native git client in [egui](https://github.com/emilk/egui), built to stay fast on the largest
repositories.

egit drives the `git` installed on your system for every repository operation, so hooks,
signing, credential helpers, and your configuration behave exactly as they do on the command
line. It loads and draws only what is on screen.

It is early work.

## Requirements

- Linux (Ubuntu 24.04 LTS or newer, or an equivalent distribution); macOS 11 or newer; or
  Windows 10 or newer. Linux is egit's home; macOS and Windows are built and tested by CI.
- git 2.43.0 or newer (on Windows, Git for Windows)

## Building

```sh
cargo build --release
```

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first use.

## In a browser

egit also runs in a browser, on a phone too. The page is egit compiled to WebAssembly, and
`egit-relay` serves it and runs git for it on the relay's computer. Your repositories, keys,
and git configuration stay on that computer; the browser only draws.

### Building it

The page runs threaded, which takes the nightly toolchain `scripts/web-env.sh` pins (it
rebuilds the standard library with atomics) and wasm-bindgen's command-line tool at the version
`Cargo.lock` names:

```sh
rustup toolchain install nightly-2026-10-10 --component rust-src --target wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
scripts/build-web.sh                 # the page, into target/web
cargo build --release -p egit-relay
```

Where `wasm-opt` (binaryen) and `brotli` are installed, the build uses them to make the page
smaller.

### Running it

```sh
target/release/egit-relay --web target/web
```

The relay listens on `127.0.0.1:7171` and prints the address to open, ending in `#` and a
token. Open it in a browser on the same computer. Open a repository from Home by its path on
the relay's computer, or name one in the address:
`http://127.0.0.1:7171/?open=/home/me/src/project#<token>`. Set `EGIT_RELAY_TOKEN` to keep
the same token, and so the same address, between runs.

The token lets the browser run programs as you on the relay's computer: git, and the shell
commands egit uses to read and write files. Keep it as private as a password, and stop the
relay when you're done.

### From another computer, or a phone

By default the relay listens only on the loopback address. To reach it from elsewhere:

- **Over ssh:** `ssh -L 7171:127.0.0.1:7171 devbox`, then open the printed address on your
  own computer.
- **Through an HTTPS proxy.** A phone needs one: browsers run threaded pages only from
  `https://` addresses or the loopback address. Tell the relay the address the proxy gives it
  with `--url`, then open the address it prints for the proxy. With
  [Tailscale](https://tailscale.com/kb/1312/serve), for example:

  ```sh
  tailscale serve --bg 7171
  target/release/egit-relay --web target/web --url https://devbox.your-tailnet.ts.net/
  ```

  In a code-server terminal, the relay finds code-server's proxy by itself and prints its
  address too.

On a phone's screen, egit shows one thing at a time: the history, then a commit you tap, then
a file. A long press opens the menu a right click opens elsewhere.

The browser needs WebGPU or WebGL2. The relay runs on Linux or macOS.

## Fonts

egit's interface is set in [Geist](https://github.com/vercel/geist-font), shipped in
`crates/egit/fonts/` under the SIL Open Font License 1.1 (`OFL-Geist.txt`).

## License

egit is licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in egit by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
