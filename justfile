# Developer entry points. Run inside the Nix shell: `nix develop -c just <recipe>`.
# The end user never runs any of this; they open the built static site.

set shell := ["bash", "-euo", "pipefail", "-c"]

wasm_crate := "lvp-wasm"
wasm_out := "web/src/wasm"
wasm_target := "--target wasm32-unknown-unknown"
# Shared-memory wasm needs a std built with atomics (see .cargo/config.toml).
build_std := "-Z build-std=std,panic_abort"
# build-std compiles the standard library with this workspace's profile. Its math routines
# (libm in compiler_builtins) rely on integer wraparound, so they must never get overflow
# checks or debug assertions, as in the prebuilt std; otherwise debug wasm traps in `hypot`.
std_profile := "--config profile.dev.package.compiler_builtins.overflow-checks=false --config profile.dev.package.compiler_builtins.debug-assertions=false"

default:
    @just --list --unsorted

# Full one-shot build of the static site into web/dist (release wasm, optimized).
build: deps (wasm "release")
    cd web && npm run typecheck && npm run build

# Dev server with hot reload (debug wasm; rebuild wasm with `just wasm` after Rust changes).
dev: deps wasm
    cd web && npm run dev

# All automated tests: Rust unit + property tests, then TypeScript tests against the built wasm.
test: deps wasm
    cargo test --workspace
    cd web && npm run typecheck && npm run test

# Browser end-to-end tests of the production bundle (needs `just build` first and a local
# Chrome; Playwright drives the installed browser, nothing is downloaded).
e2e:
    cd web && npx playwright test

# Fixture-based regression tests (slow / need fixtures/); these are `#[ignore]`d in `just test`.
regression:
    cargo test --workspace -- --ignored

# Fuzz one target (dxf gerber svg pdf gds ipc pipeline) for `secs` seconds, seeded from the
# fixtures; the corpus grows in fuzz/corpus/<target>. Crashers land in fuzz/artifacts/<target>/:
# minimise with `cd fuzz && cargo fuzz tmin --sanitizer none -O <target> <file>` and save the result under
# fixtures/fuzz/<target>/, where `just test` replays it.
# Extra arguments go to libFuzzer, e.g. `-fork=2 -ignore_crashes=1` to keep going after a crash.
fuzz target secs="60" *args: (fuzz-seeds target)
    cd fuzz && cargo fuzz run --sanitizer none -O {{target}} corpus/{{target}} seeds/{{target}} \
        -- -max_total_time={{secs}} -timeout=10 -rss_limit_mb=2048 -print_final_stats=1 {{args}}

# Every fuzz target in turn.
fuzz-all secs="60":
    for t in dxf gerber svg pdf gds ipc pipeline; do just fuzz "$t" {{secs}}; done

# Seed inputs per target from fixtures/. The pipeline target's first byte selects the file
# name and preset (lvp_core::fuzz::pipeline), so its seeds are fixtures behind such a byte.
[private]
fuzz-seeds target:
    #!/usr/bin/env bash
    set -euo pipefail
    dir=fuzz/seeds/{{target}}
    rm -rf "$dir" && mkdir -p "$dir" fuzz/corpus/{{target}}
    case {{target}} in
      dxf|gerber|svg|pdf|gds) cp fixtures/{{target}}/* "$dir"/ ;;
      ipc) cp fixtures/ipc2581/* "$dir"/ ;;
      pipeline)
        # selector % 6 = name index (dxf gbr svg pdf gds xml), (selector / 8) % 4 = preset.
        i=0
        for sub in dxf gerber svg pdf gds ipc2581; do
          for f in fixtures/$sub/*; do
            for preset in 0 1 2 3; do
              for s in $(seq 0 255); do
                if (( s % 6 == i && (s / 8) % 4 == preset )); then break; fi
              done
              { printf "\\$(printf %03o "$s")"; cat "$f"; } > "$dir/$sub-$(basename "$f")-p$preset"
            done
          done
          i=$((i + 1))
        done ;;
      *) echo "unknown fuzz target {{target}}" >&2; exit 1 ;;
    esac

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
    cargo build -p {{wasm_crate}} {{wasm_target}} {{build_std}} {{std_profile}} {{ if profile == "release" { "--release" } else { "" } }}
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
