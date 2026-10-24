{
  description = "Laser Vector Prep - browser-only vector/toolpath preprocessor for galvo lasers";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust. nixpkgs' rustc bundles the wasm32-unknown-unknown std, so no overlay is needed.
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer
            lld # nixpkgs' rustc links wasm32 with the system lld, not a bundled rust-lld
            # Rust -> JS glue. Its version must equal the `wasm-bindgen` crate in Cargo.lock
            # (`just check-wasm-bindgen` enforces this).
            wasm-bindgen-cli
            binaryen # wasm-opt
            # Web
            nodejs_24
            # Task runner
            just
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

          shellHook = ''
            echo "lvp dev shell: rustc $(rustc --version | cut -d' ' -f2), node $(node --version), wasm-bindgen $(wasm-bindgen --version | cut -d' ' -f2)"
          '';
        };
      });
    };
}
