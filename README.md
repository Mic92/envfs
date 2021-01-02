# Envfs

Fuse filesystem that returns symlinks to executables based on the PATH of the requesting process. This is useful to execute shebangs on NixOS that assume hard coded locations in locations like /bin or /usr/bin etc.

## Usage

```console
$ envfs /usr/bin
$ mount --bind /usr/bin /bin
```

## TODO

* Provide fallback PATH for /bin/sh and /usr/bin/env if the process is a setuid
binary or has no PATH set.
* Disable symlink caching in cntr-fuse
* NixOS module
