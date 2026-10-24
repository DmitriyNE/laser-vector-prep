# Laser Vector Prep

Browser-only vector artwork and toolpath preprocessor for galvo laser software.
Import DXF/Gerber/SVG/PDF, compose working layers (booleans, negative regions),
generate fill/trace paths, and export a 1:1 color-separated vector PDF. Nothing
leaves the browser.

- License: Apache-2.0 ([`LICENSE`](LICENSE))
- Requirements: [`docs/spec.md`](docs/spec.md)
- Building and running: [`docs/development.md`](docs/development.md)
- Engineering log, todo, decisions, defects: [`docs/engineering/`](docs/engineering/)

Quick start (needs [Nix](https://nixos.org) with flakes):

```sh
nix develop -c just build   # static site in web/dist
nix develop -c just dev     # dev server
nix develop -c just test    # Rust + TypeScript tests
nix develop -c just fixtures  # regenerate fixtures/ from the generator
```
