{
  description = "Laser Vector Prep - browser-only vector/toolpath preprocessor for galvo lasers";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # Provides the date-pinned nightly from rust-toolchain.toml (nixpkgs has no nightly).
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      }));
    in
    {
      devShells = forAll (pkgs:
        let
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          # Playwright's own Firefox build for the browser tests (Chrome is the installed
          # one). Its revision must match the npm @playwright/test version
          # (`just check-playwright` enforces this).
          playwrightFirefox = pkgs.playwright-driver.browsers.override {
            withChromium = false;
            withChromiumHeadlessShell = false;
            withWebkit = false;
            withFfmpeg = false;
          };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rust # rustc, cargo, clippy, rustfmt, rust-analyzer, rust-src, wasm32 target
              # Rust -> JS glue. Its version must equal the `wasm-bindgen` crate in Cargo.lock
              # (`just check-wasm-bindgen` enforces this).
              wasm-bindgen-cli
              binaryen # wasm-opt
              cargo-fuzz # `just fuzz` (libFuzzer targets in fuzz/)
              nodejs_24
              just
            ];

            RUST_SRC_PATH = "${rust}/lib/rustlib/src/rust/library";
            PLAYWRIGHT_BROWSERS_PATH = "${playwrightFirefox}";
            PLAYWRIGHT_NIX_VERSION = pkgs.playwright-driver.version;

            shellHook = ''
              echo "lvp dev shell: $(rustc --version | cut -d' ' -f2-4), node $(node --version), wasm-bindgen $(wasm-bindgen --version | cut -d' ' -f2)"
            '';
          };
        });
    };
}
