{
  description = "Fuse filesystem that returns symlinks to executables based on the PATH of the requesting process.";

  inputs.utils.url = "github:numtide/flake-utils";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs";

  outputs = { self, nixpkgs, utils }: {
    nixosModules.envfs = import ./modules/envfs.nix;
  } // utils.lib.eachSystem ["x86_64-linux" "aarch64-linux"] (system: let
    pkgs = nixpkgs.legacyPackages.${system};
  in {
    packages.envfs = pkgs.callPackage ./default.nix {
      packageSrc = self;
    };
    defaultPackage = self.packages.${system}.envfs;
  }) // {
    checks.x86_64-linux.integration-tests = let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in import ./nixos-test.nix {
        makeTest = import (nixpkgs + "/nixos/tests/make-test-python.nix");
        inherit pkgs;
        inherit (self.packages.${system}) cntr;
    };
  };
}
