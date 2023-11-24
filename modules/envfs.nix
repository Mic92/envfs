{ pkgs, config, lib, ... }:

let
  mounts = {
    "/usr/bin" = {
      device = "none";
      fsType = "envfs";
      options = [
        "fallback-path=${pkgs.runCommand "fallback-path" {} ''
          mkdir -p $out
          ln -s ${config.environment.usrbinenv} $out/env
          ln -s ${config.environment.binsh} $out/sh
        ''}"
        "nofail"
      ];
    };
    "/bin" = {
      device = "/usr/bin";
      fsType = "none";
      options = [ "bind" "nofail" ];
    };
  };
in
{
  environment.systemPackages = [ (pkgs.callPackage ../. { }) ];
  fileSystems = if config.virtualisation ? qemu then lib.mkVMOverride mounts else mounts;

  system.activationScripts.usrbinenv = lib.mkForce "";
  system.activationScripts.binsh = lib.mkForce "";
}
