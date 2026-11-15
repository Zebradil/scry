{
  description = "scry: preview what a change to a Nix fleet does to each machine";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

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
      # Dependencies build in their own derivation from crane's dummy source.
      # crane keeps scry's own version in that source, so a release bump would
      # still rebuild every dependency; it is stripped from the dummy Cargo.toml
      # and Cargo.lock (both then read 0.0.0) so only scry recompiles. Cargo.lock
      # is left out of `depsSrc` because crane copies it from there verbatim.
      version = (lib.importTOML ./Cargo.toml).package.version;
      lockVersionLine = v: "name = \"scry\"\nversion = \"${v}\"\n";
      lock = builtins.readFile ./Cargo.lock;
      depsLock = builtins.replaceStrings [ (lockVersionLine version) ] [ (lockVersionLine "0.0.0") ] lock;
      depsSrc = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./build.rs
          ./src
          ./tests
        ];
      };
      mkCrane =
        pkgs:
        let
          craneLib = crane.mkLib pkgs;
          cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };
          cargoArtifacts =
            assert lib.assertMsg (depsLock != lock) "scry's version not found in Cargo.lock";
            craneLib.buildDepsOnly {
              pname = "scry";
              version = "0.0.0";
              inherit cargoVendorDir;
              dummySrc = craneLib.mkDummySrc {
                src = depsSrc;
                cargoLock = builtins.toFile "Cargo.lock" depsLock;
                cleanCargoTomlFilter =
                  path:
                  craneLib.filters.cargoTomlDefault path
                  &&
                    path != [
                      "package"
                      "version"
                    ];
              };
            };
        in
        {
          inherit craneLib cargoArtifacts cargoVendorDir;
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
        default = (mkCrane pkgs).craneLib.buildPackage ({
          inherit src;
          inherit (mkCrane pkgs) cargoArtifacts cargoVendorDir;
          # The sandbox has no .git, so build.rs reads the revision from here (spec 4.6).
          # Only this derivation carries it: on clippy or the dependency build it would
          # make every commit a full rebuild.
          env.SCRY_GIT_REV = self.shortRev or self.dirtyShortRev or "unknown";
          nativeBuildInputs = [ pkgs.installShellFiles ];
          # `tests/cli.rs` drives the bash completion script, with its `complete` builtin.
          nativeCheckInputs = [ pkgs.bashInteractive ];
          # The scripts come from the built binary, which a cross build cannot run.
          postInstall = lib.optionalString (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
            installShellCompletion --cmd scry \
              --bash <($out/bin/scry completions bash) \
              --zsh <($out/bin/scry completions zsh) \
              --fish <($out/bin/scry completions fish)
          '';
          meta = {
            description = "Preview what a change to a Nix fleet does to each machine";
            mainProgram = "scry";
            license = with lib.licenses; [
              mit
              asl20
            ];
          };
        });
      });

      # CI builds and publishes `checks.<system>` as one set, so the package and dev shell are here too.
      checks = forAllSystems (
        pkgs:
        let
          inherit (mkCrane pkgs) craneLib cargoArtifacts cargoVendorDir;
        in
        {
          build = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          devShell = self.devShells.${pkgs.stdenv.hostPlatform.system}.default;
          clippy = craneLib.cargoClippy {
            inherit src cargoArtifacts cargoVendorDir;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          };
          # CI pushes only the output closures of these checks, and the dependency
          # build is a build-time input of them, so without this it never reaches
          # the cache and every version bump recompiles it.
          build-inputs = pkgs.linkFarm "scry-build-inputs" {
            deps = cargoArtifacts;
            crane-utils = craneLib.craneUtils;
          };
        }
      );

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

      # `nix run .#web-demo -- OUT_DIR`: the browser viewer (web/) on the fixture's report, for the
      # docs site's prebuild. Outside the sandbox: trunk fetches the wasm-bindgen matching
      # web/Cargo.lock, which nixpkgs rarely has.
      apps = forAllSystems (pkgs: {
        web-demo = {
          type = "app";
          program = lib.getExe (
            pkgs.writeShellApplication {
              name = "web-demo";
              runtimeInputs = [
                pkgs.cargo
                pkgs.rustc
                pkgs.lld
                pkgs.trunk
                pkgs.coreutils
                pkgs.git
              ];
              text = ''
                out=$(realpath -m "''${1:?usage: web-demo OUT_DIR}")
                cd "$(git rev-parse --show-toplevel)"
                # Exit 1 stands for the fixture's failing configurations; the report is complete.
                SCRY_FIXTURE=1 cargo run --release --quiet -- colmena --format json >web/report.json || [ $? -eq 1 ]
                cd web
                trunk build --release --public-url ./ --dist "$out"
              '';
            }
          );
        };
      });

      formatter = forAllSystems treefmt;
    };
}
