{ lib, inputs, ... }: {
  imports = [
    inputs.treefmt-nix.flakeModule
  ];

  perSystem = { pkgs, ... }: {
    treefmt = {
      # Used to find the project root
      projectRootFile = "flake.lock";

      programs.deno.enable = !pkgs.stdenv.buildPlatform.isRiscV64;
      programs.rustfmt.enable = true;

      settings.formatter.nix = {
        command = "sh";
        options = [
          "-eucx"
          ''
            # First deadnix
            ${lib.getExe pkgs.deadnix} --edit "$@"
            # Then nixpkgs-fmt
            ${lib.getExe pkgs.nixpkgs-fmt} "$@"
          ''
          "--"
        ];
        includes = [ "*.nix" ];
        excludes = [ "nix/sources.nix" ];
      };
    };
  };
}
