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
        "PATH=${pkgs.coreutils}/bin /usr/bin/cp --version",
        # check fallback paths
        "PATH= /usr/bin/sh --version",
        "PATH= /usr/bin/env --version",
        # no stat
        "! test -e /usr/bin/cp",
        # also picks up PATH that was set after execve
        "! /usr/bin/hello",
        "PATH=${pkgs.hello}/bin /usr/bin/hello",
    )
  '';
} {
  inherit pkgs;
  inherit (pkgs) system;
}
