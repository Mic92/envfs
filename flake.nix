{
  description = "Fuse filesystem that returns symlinks to executables based on the PATH of the requesting process.";

  inputs.nixpkgs.url = "git+https://github.com/NixOS/nixpkgs?shallow=1&ref=nixpkgs-unstable";

  inputs.flake-parts.url = "github:hercules-ci/flake-parts";
  inputs.flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
  inputs.treefmt-nix.url = "github:numtide/treefmt-nix";
  inputs.treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } ({ self, ... }: {
      imports = [ ./treefmt.nix ];
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "riscv64-linux"
        "aarch64-darwin"
      ];
      flake.nixosModules.envfs = import ./modules/envfs.nix;
      perSystem = { lib, config, pkgs, ... }: {
        packages = lib.optionalAttrs pkgs.stdenv.isLinux {
          envfs = pkgs.callPackage ./default.nix {
            packageSrc = self;
          };
          default = config.packages.envfs;
        };
        checks =
          let
            packages = lib.mapAttrs' (n: lib.nameValuePair "package-${n}") config.packages;
            devShells = lib.mapAttrs' (n: lib.nameValuePair "devShell-${n}") config.devShells;
          in
          packages // devShells // lib.optionalAttrs pkgs.stdenv.isLinux {
            envfsCrossAarch64 = pkgs.pkgsCross.aarch64-multiplatform.callPackage ./default.nix {
              packageSrc = self;
            };

            clippy = config.packages.envfs.override { enableClippy = true; };
            # disable riscv64 for now until https://github.com/NixOS/nixpkgs/pull/393093 is merged
          } // lib.optionalAttrs (pkgs.stdenv.isLinux && !pkgs.stdenv.hostPlatform.isRiscV) {
            integration-tests = pkgs.callPackage ./nixos-test.nix { };
          };
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.rustc
            pkgs.cargo
            pkgs.cargo-watch
            pkgs.cargo-edit
            pkgs.clippy
          ];
        };
      };
    });
}
