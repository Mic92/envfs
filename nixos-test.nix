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
        # we only get stat() for fallback paths
        "PATH=${pkgs.coreutils}/bin ${pkgs.runtimeShell} -c '[[ ! -f /usr/bin/cp ]]'",
        "PATH= ${pkgs.runtimeShell} -c '[[ -f /usr/bin/sh ]]'",
    )
  '';
} {
  inherit pkgs;
  inherit (pkgs) system;
}
