{ flake ? builtins.getFlake (toString ./.)
, pkgs ? flake.inputs.nixpkgs.legacyPackages.${builtins.currentSystem}
, makeTest ? pkgs.callPackage (flake.inputs.nixpkgs + "/nixos/tests/make-test-python.nix")
, cntr ? flake.defaultPackage.${builtins.currentSystem}
}:
makeTest {
  name = "envfs";
  nodes.machine = import ./nixos-example.nix;

  testScript = ''
    start_all()
    machine.succeed(
        "PATH=${pkgs.coreutils}/bin ${pkgs.runtimeShell} -c '/usr/bin/cp --version'",
        # check fallback paths
        "PATH= ${pkgs.runtimeShell} -c '/usr/bin/sh --version'",
        "PATH= ${pkgs.runtimeShell} -c '/usr/bin/env --version'",
    )
  '';
} {
  inherit pkgs;
  inherit (pkgs) system;
}
