# Developer entry points. Run inside the Nix shell: `nix develop -c just <recipe>`.
# The end user never runs any of this; they open the built static site.

set shell := ["bash", "-euo", "pipefail", "-c"]

wasm_crate := "lvp-wasm"
wasm_out := "web/src/wasm"
wasm_target := "--target wasm32-unknown-unknown"
# Shared-memory wasm needs a std built with atomics (see .cargo/config.toml).
build_std := "-Z build-std=std,panic_abort"

default:
    @just --list --unsorted

# Full one-shot build of the static site into web/dist (release wasm, optimized).
build: deps (wasm "release")
    cd web && npm run typecheck && npm run build

# Dev server with hot reload (debug wasm; rebuild wasm with `just wasm` after Rust changes).
dev: deps wasm
    cd web && npm run dev

# All automated tests: Rust unit + property tests, then TypeScript tests against the built wasm.
test: wasm
    cargo test --workspace
    cd web && npm run typecheck && npm run test

# Fixture-based regression tests (slow / need fixtures/); these are `#[ignore]`d in `just test`.
regression:
    cargo test --workspace -- --ignored

# Regenerate fixtures/ from the deterministic generator (a test fails if they are stale).
fixtures:
    cargo run -p lvp-fixtures

# Formatting and lints.
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p {{wasm_crate}} {{wasm_target}} {{build_std}} -- -D warnings

fmt:
    cargo fmt --all

# Compile the wasm crate and generate JS/TS glue into web/src/wasm. profile: dev | release
wasm profile="dev": check-wasm-bindgen
    cargo build -p {{wasm_crate}} {{wasm_target}} {{build_std}} {{ if profile == "release" { "--release" } else { "" } }}
    mkdir -p {{wasm_out}}
    wasm-bindgen target/wasm32-unknown-unknown/{{ if profile == "release" { "release" } else { "debug" } }}/lvp_wasm.wasm \
        --out-dir {{wasm_out}} --target web --typescript
    # wasm-bindgen-rayon's pool worker imports the glue as a package (`../../..`).
    printf '{ "name": "lvp-wasm", "private": true, "type": "module", "main": "lvp_wasm.js", "types": "lvp_wasm.d.ts" }\n' > {{wasm_out}}/package.json
    {{ if profile == "release" { "wasm-opt -O3 -o " + wasm_out + "/lvp_wasm_bg.wasm " + wasm_out + "/lvp_wasm_bg.wasm" } else { "true" } }}

# wasm-bindgen-cli (from nixpkgs) and the wasm-bindgen crate (Cargo.lock) must match exactly.
check-wasm-bindgen:
    @cli="$(wasm-bindgen --version | awk '{print $2}')"; \
    lock="$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/"/, "", $3); print $3}' Cargo.lock)"; \
    if [ "$cli" != "$lock" ]; then \
        echo "wasm-bindgen version mismatch: cli=$cli crate=$lock. Bump Cargo.toml and/or flake.lock together." >&2; exit 1; \
    fi

# npm ci, skipped when node_modules is already up to date with the lockfile.
deps:
    cd web && if [ ! -f node_modules/.package-lock.json ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then npm ci; fi

# Serve the production build locally (checks the service-worker path, not just the dev server headers).
preview: build
    cd web && npm run preview

clean:
    rm -rf target web/dist {{wasm_out}}
