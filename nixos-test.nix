{ testers
, hello
, coreutils
, writeScript
, bash
, python3
, fish
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

  # The reader sleeps past the attribute TTL and uses an inherited fd, so no
  # path walk refreshes the envfs root's attributes. A deadlocked reader can't
  # be killed, so the script doesn't wait for it.
  readlinkFromOtherProcess = writeScript "readlink-from-other-process" ''
    #!${python3}/bin/python3
    import os
    import subprocess
    import sys
    import time

    if len(sys.argv) == 1:
        fd = os.open("/usr/bin/env", os.O_PATH | os.O_NOFOLLOW)
        reader = subprocess.Popen(
            [sys.argv[0], str(fd)],
            pass_fds=[fd],
            env={"PATH": "/opt/bin"},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        try:
            out, _ = reader.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            sys.exit("readlink did not return within 10 seconds")
        sys.stdout.write(out.decode())
        sys.exit(reader.returncode)
    else:
        time.sleep(2)
        print(os.readlink("", dir_fd=int(sys.argv[1])))
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

    # fish stats commands before executing them (https://github.com/Mic92/envfs/issues/173)
    machine.succeed("PATH=${coreutils}/bin ${fish}/bin/fish -c '/usr/bin/cp --version'")
    # other programs must not see /usr/bin entries via stat: podman probes
    # /usr/bin/conmon and then execs it without PATH (https://github.com/Mic92/envfs/issues/180)
    machine.fail(
        "PATH=${coreutils}/bin stat /usr/bin/cp",
        "PATH=${coreutils}/bin test -e /usr/bin/cp",
    )

    out = machine.succeed("PATH=${coreutils}/bin readlink /usr/bin/cp")
    assert out.startswith("${coreutils}/bin/"), out

    out = machine.succeed("PATH=${python3}/bin ${pythonShebang}")
    print(out)
    assert out == "OK\n"

    out = machine.succeed("PATH=${bash}/bin ${bashShebang}")
    print(out)
    assert out == "OK\n"

    # relative PATH entries are relative to the caller's working directory
    machine.succeed(
        "mkdir -p /tmp/cwd/bin",
        "ln -s ${hello}/bin/hello /tmp/cwd/bin/hello",
    )
    out = machine.succeed("cd /tmp/cwd && PATH=bin /usr/bin/hello")
    assert out == "Hello, world!\n", out

    # PATH entries are looked up in the caller's mount namespace
    machine.succeed("mkdir -p /opt/tools")
    out = machine.succeed(
        "unshare --mount sh -c 'mount --bind ${hello}/bin /opt/tools && PATH=/opt/tools /usr/bin/hello'"
    )
    assert out == "Hello, world!\n", out

    # PATH entries are looked up with the caller's permissions
    machine.succeed("install -D -m 755 /dev/null /root/bin/private-tool")
    out = machine.succeed("PATH=/root/bin ${coreutils}/bin/readlink /usr/bin/private-tool")
    assert out == "/root/bin/private-tool\n", out
    machine.fail(
        "runuser -u nobody -- env PATH=/root/bin ${coreutils}/bin/readlink /usr/bin/private-tool"
    )

    # a bind mount of /usr/bin that envfs was not told about must not make envfs
    # wait on its own filesystem
    machine.succeed("mkdir -p /opt/bin", "mount --bind /usr/bin /opt/bin")
    out = machine.succeed("${readlinkFromOtherProcess}")
    assert out.startswith("/nix/store/"), out
    machine.succeed("umount /opt/bin")

    # A daemon left over from a previous mount must not unmount its successor
    # (https://github.com/Mic92/envfs/issues/238).
    old_pid = machine.succeed("pgrep -x mount.envfs").strip()
    machine.succeed("systemctl restart usr-bin.mount")
    machine.wait_until_succeeds("mountpoint -q /usr/bin/")
    # The old daemon exits once its FUSE session ends
    machine.wait_until_fails(f"kill -0 {old_pid}", timeout=30)
    machine.succeed("PATH= test -e /usr/bin/env")
  '';
}
