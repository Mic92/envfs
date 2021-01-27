# Envfs

Fuse filesystem that returns symlinks to executables based on the PATH of the requesting process. 
This is useful to execute shebangs on NixOS that assume hard coded locations in locations like /bin or /usr/bin etc.

## Demo

```console
$ ls -l /usr/bin/{bash,python}
lr----x--t 0 root  1 Jan  1970  /usr/bin/bash -> /nix/store/j37555sj2w3xsql3f8qrwbaim7pv67hg-bash-interactive-4.4-p23/bin/bash
lr----x--t 0 root  1 Jan  1970  /usr/bin/python -> /home/joerg/.nix-profile/bin/python
$ cat > foo.py <<EOF
#!/usr/bin/python
print("hello world")
EOF
$ chmod +x ./foo.py && ./foo.py
```

## Installation in NixOS

Choose one of the following methods:

### [niv](https://github.com/nmattia/niv) (Current recommendation)
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
  
### Flakes

If you use experimental nix flakes support:

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

## Build and run from source

```console
$ nix-build
$ sudo ./result/bin/envfs /usr/bin
$ mount --bind /usr/bin /bin
```
