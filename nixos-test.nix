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
        "PATH=${pkgs.coreutils}/bin readlink /usr/bin/sh /usr/bin/cp",
        # test fallback paths works
        "PATH= ${pkgs.coreutils}/bin/readlink /usr/bin/sh /usr/bin/env /bin/sh",
    )
  '';
} {
  inherit pkgs;
  inherit (pkgs) system;
}
