# Envfs

Fuse filesystem that returns symlinks to executables based on the PATH of the requesting process. This is useful to execute shebangs on NixOS that assume hard coded locations in locations like /bin or /usr/bin etc.

## Usage

```console
$ envfs /usr/bin
$ mount --bind /usr/bin /bin
$ ls -l /usr/bin/{bash,python}
lr----x--t 0 root  1 Jan  1970  /usr/bin/bash -> /nix/store/j37555sj2w3xsql3f8qrwbaim7pv67hg-bash-interactive-4.4-p23/bin/bash
lr----x--t 0 root  1 Jan  1970  /usr/bin/python -> /home/joerg/.nix-profile/bin/python
```

## TODO

* Provide fallback PATH for /bin/sh and /usr/bin/env if the process is a setuid
binary or has no PATH set.
* Disable symlink caching in cntr-fuse
* NixOS module
