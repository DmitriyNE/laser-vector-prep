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
      # `nix build`: the static site, built hermetically by the same `just build` developers
      # run. Cargo sources are vendored from Cargo.lock and from the standard library's own
      # lockfile (build-std compiles std for the shared-memory wasm target); npm sources come
      # from package-lock.json: update the `npmDeps` hash when web/package-lock.json changes (the
      # build prints the new one).
      packages = forAll (pkgs:
        let
          lib = pkgs.lib;
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          stdLock = "${rust}/lib/rustlib/src/rust/library/Cargo.lock";
          vendored = pkgs.symlinkJoin {
            name = "lvp-cargo-vendor";
            paths = [
              (pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; })
              (pkgs.rustPlatform.importCargoLock { lockFile = stdLock; })
            ];
          };
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          site = pkgs.stdenv.mkDerivation {
            pname = "laser-vector-prep-site";
            inherit version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./.cargo
                ./rust-toolchain.toml
                ./justfile
                ./LICENSE
                ./crates
                (lib.fileset.difference ./web (lib.fileset.unions [
                  (lib.fileset.maybeMissing ./web/node_modules)
                  (lib.fileset.maybeMissing ./web/dist)
                  (lib.fileset.maybeMissing ./web/src/wasm)
                ]))
              ];
            };
            npmRoot = "web";
            npmDeps = pkgs.fetchNpmDeps {
              src = ./web;
              hash = "sha256-K84GR2fH7dUrXFQWXaRjj/40u/im+8fPHK89+6QW8L0=";
            };
            nativeBuildInputs = [
              rust
              pkgs.wasm-bindgen-cli
              pkgs.binaryen
              pkgs.nodejs_24
              pkgs.npmHooks.npmConfigHook
              pkgs.just
            ];
            CARGO_NET_OFFLINE = "true";
            configurePhase = ''
              runHook preConfigure
              export CARGO_HOME="$TMPDIR/cargo-home"
              mkdir -p "$CARGO_HOME"
              cat > "$CARGO_HOME/config.toml" <<EOF
              [source.crates-io]
              replace-with = "vendored-sources"
              [source.vendored-sources]
              directory = "${vendored}"
              EOF
              runHook postConfigure
            '';
            buildPhase = ''
              runHook preBuild
              just build
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              cp -R web/dist "$out"
              cp LICENSE "$out/"
              runHook postInstall
            '';
          };
        in
        {
          default = site;
          inherit site;
        });

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
              git # `just package` names the tarball after the commit
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
