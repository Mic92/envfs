# Envfs

Fuse filesystem that returns symlinks to executables based on the PATH of the
requesting process.  This is useful to execute shebangs on NixOS that assume
hard coded locations in locations like /bin or /usr/bin etc.

## Demo

Mount envfs on /usr/bin
`sudo envfs /usr/bin`

Programs are magically available, based on the PATH of the calling process.
`$ /usr/bin/env --version`
```
env (GNU coreutils) 9.1
Packaged by https://nixos.org
Copyright (C) 2022 Free Software Foundation, Inc.
License GPLv3+: GNU GPL version 3 or later <https://gnu.org/licenses/gpl.html>.
This is free software: you are free to change and redistribute it.
There is NO WARRANTY, to the extent permitted by law.

Written by Richard Mlynarik, David MacKenzie, and Assaf Gordon.
```

`$ echo $PATH`

If the caller doesn't have the binary in the PATH, it will fail.
`$ PATH= /usr/bin/env --version 2>&1 || true`

As you can see, /usr/bin is empty.
`$ ls -la /usr/bin`
```
total 1
drwxr-xr-x 3345300086 root root 0 Jan  1  1970 .
drwxr-xr-x          3 root root 3 Sep  5 13:04 ..
```

By default, binaries are only available whenever the calling process executes or open
a program, not when using stat or listing the directory:
`$ ls -la /usr/bin/env 2>&1 || true`
```
ls: cannot access '/usr/bin/env': No such file or directory
```

This behaviour can be overridden by setting `ENVFS_RESOLVE_ALWAYS=1`.
`$ ENVFS_RESOLVE_ALWAYS=1 ls -la /usr/bin/env`
```
lr----x--t 1 root root 0 Jan  1  1970 /usr/bin/env -> /nix/store/4vjigg3pr8bns6id4af51mza5p73l9lx-coreutils-9.1/bin/env
```

In conclusion, combined with the usual Nix wrappers or nix-shells, it makes things magically
work!

## Installation in NixOS

Choose one of the following methods:


### In NixOS starting with 23.05 (Current recommendation)


Since NixOS 23.05 you can enable envfs with a single line:

```nix
{
  services.envfs.enable = true;
}
```

### Flakes

If you use nix flakes support:

``` nix
{
  inputs.envfs.url = "github:Mic92/envfs";
  inputs.envfs.inputs.nixpkgs.follows = "nixpkgs";
  
  outputs = { self, nixpkgs, envfs }: {
    # change `yourhostname` to your actual hostname
    nixosConfigurations.yourhostname = nixpkgs.lib.nixosSystem {
      # change to your system:
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        envfs.nixosModules.envfs
      ];
    };
  };
}
```

### [niv](https://github.com/nmattia/niv)
  First add it to niv:
  
```console
$ niv add Mic92/envfs
```

  Then add the following to your configuration.nix in the `imports` list:
  
```nix
{
  imports = [ "${(import ./nix/sources.nix).envfs}/modules/envfs.nix" ];
}
```
  
### nix-channel

  As root run:
  
```console
$ nix-channel --add https://github.com/Mic92/envfs/archive/main.tar.gz envfs
$ nix-channel --update
```
  
  Then add the following to your configuration.nix in the `imports` list:
  
```nix
{
  imports = [ <envfs/modules/envfs.nix> ];
}
```

### fetchTarball

  Add the following to your configuration.nix:

``` nix
{
  imports = [ "${builtins.fetchTarball "https://github.com/Mic92/envfs/archive/main.tar.gz"}/modules/envfs.nix" ];
}
```
  
  or with pinning:
  
```nix
{
  imports = let
    # replace this with an actual commit id or tag
    commit = "f2783a8ef91624b375a3cf665c3af4ac60b7c278";
  in [ 
    "${builtins.fetchTarball {
      url = "https://github.com/Mic92/envfs/archive/${commit}.tar.gz";
      # replace this with an actual hash
      sha256 = "0000000000000000000000000000000000000000000000000000";
    }}/modules/envfs.nix"
  ];
}
```
  

## Build and run from source

```console
$ nix-build
$ sudo ./result/bin/envfs /usr/bin
$ mount --bind /usr/bin /bin
```
