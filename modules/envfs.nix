{ pkgs, config, ... }:
{
  environment.systemPackages = [ (pkgs.callPackage ../.) ];
  fileSystems = {
    "/usr/bin" = {
      fsType = "fuse.envfs";
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
}
