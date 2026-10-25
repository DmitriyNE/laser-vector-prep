# Laser Vector Prep

[![CI](https://github.com/DmitriyNE/laser-vector-prep/actions/workflows/ci.yml/badge.svg)](https://github.com/DmitriyNE/laser-vector-prep/actions/workflows/ci.yml)

**Open the app: <https://dmitriyne.github.io/laser-vector-prep/>** (runs entirely in the
browser; files never leave your machine). Works in current Chrome, Edge and Firefox;
Safari is not supported.

Browser-only vector artwork and toolpath preprocessor for galvo laser software.
Import DXF, Gerber, SVG, vector PDF, GDSII or IPC-2581, compose working layers
(booleans, negative regions), generate fill/trace/hatch paths, plan the marking order,
and export a 1:1 colour-separated vector PDF (or SVG/DXF). Nothing leaves the browser.

- License: Apache-2.0 ([`LICENSE`](LICENSE))
- Requirements: [`docs/spec.md`](docs/spec.md)
- Supported file formats: [`docs/formats.md`](docs/formats.md)
- Building and running: [`docs/development.md`](docs/development.md)
- Engineering log, todo, decisions, defects: [`docs/engineering/`](docs/engineering/)

Quick start (needs [Nix](https://nixos.org) with flakes):

```sh
nix develop -c just build   # static site in web/dist
nix develop -c just dev     # dev server
nix develop -c just test    # Rust + TypeScript tests
nix develop -c just e2e     # browser tests in Chrome and Firefox (after just build)
nix develop -c just fixtures  # regenerate fixtures/ from the generator
```
