{ pkgs, config, lib, ... }:

let
  mounts = {
    "/usr/bin" = {
      device = "envfs";
      fsType = "envfs";
      options = [
        "fallback-path=${pkgs.runCommand "fallback-path" {} ''
          mkdir -p $out
          ln -s ${pkgs.coreutils}/bin/env $out/env
          ln -s ${config.system.build.binsh}/bin/sh $out/sh
        ''}"
      ];
    };
    "/bin" = {
      device = "/usr/bin";
      fsType = "none";
      options = [ "bind" ];
    };
  };
in {
  environment.systemPackages = [ (pkgs.callPackage ../. {}) ];
  fileSystems = if config.virtualisation ? qemu then lib.mkVMOverride mounts else mounts;
}
