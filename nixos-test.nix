{ testers
, hello
, coreutils
, writeScript
, bash
, python3
,
}:
let
  pythonShebang = writeScript "python-shebang" ''
    #!/usr/bin/python
    print("OK")
  '';

  bashShebang = writeScript "bash-shebang" ''
    #!/usr/bin/bash
    echo "OK"
  '';
in
testers.runNixOSTest {
  name = "envfs";
  nodes.machine = import ./nixos-example.nix;

  testScript = ''
    start_all()
    machine.wait_until_succeeds("mountpoint -q /usr/bin/")
    machine.succeed(
        "PATH=${coreutils}/bin /usr/bin/cp --version",
        # check fallback paths
        "PATH= /usr/bin/sh --version",
        "PATH= /usr/bin/env --version",
        "PATH= test -e /usr/bin/sh",
        "PATH= test -e /usr/bin/env",
        # Check bind mount
        "PATH= /bin/sh --version",
        "PATH=/usr/bin:/bin /bin/sh --version",
        # also picks up PATH that was set after execve
        "! /usr/bin/hello",
        "PATH=${hello}/bin /usr/bin/hello",
    )

    out = machine.succeed("PATH=${python3}/bin ${pythonShebang}")
    print(out)
    assert out == "OK\n"

    out = machine.succeed("PATH=${bash}/bin ${bashShebang}")
    print(out)
    assert out == "OK\n"
  '';
}
