{
  description = "scry: preview what a change to a Nix fleet does to each machine";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Shared by the package and the clippy check, so both use the same vendored,
      # network-free cargo registry.
      cargoArgs = {
        pname = "scry";
        version = (lib.importTOML ./Cargo.toml).package.version;
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./build.rs
            ./src
            ./tests
          ];
        };
        cargoLock.lockFile = ./Cargo.lock;
        # The sandbox has no .git, so build.rs reads the revision from here (spec 4.6).
        env.SCRY_GIT_REV = self.shortRev or self.dirtyShortRev or "unknown";
      };

      treefmt =
        pkgs:
        pkgs.writeShellApplication {
          name = "treefmt";
          runtimeInputs = [
            pkgs.treefmt
            pkgs.nixfmt
            pkgs.rustfmt
          ];
          text = ''exec treefmt "$@"'';
        };
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage (
          cargoArgs
          // {
            # Shell completions for bash, zsh and fish get installed here (M8):
            # installShellFiles in nativeBuildInputs, installShellCompletion in postInstall.
            meta = {
              description = "Preview what a change to a Nix fleet does to each machine";
              mainProgram = "scry";
            };
          }
        );
      });

      checks = forAllSystems (pkgs: {
        clippy = pkgs.rustPlatform.buildRustPackage (
          cargoArgs
          // {
            pname = "scry-clippy";
            nativeBuildInputs = [ pkgs.clippy ];
            buildPhase = "cargo clippy --all-targets --offline -- -D warnings";
            doCheck = false;
            installPhase = "touch $out";
          }
        );
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.cargo-insta
            (treefmt pkgs)
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      formatter = forAllSystems treefmt;
    };
}
