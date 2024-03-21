{ pkgs, config, lib, ... }:

let
  mounts = {
    "/usr/bin" = {
      device = "none";
      fsType = "envfs";
      options = [
        "bind-mount=/bin"
        "fallback-path=${pkgs.runCommand "fallback-path" {} ''
          mkdir -p $out
          ln -s ${config.environment.usrbinenv} $out/env
          ln -s ${config.environment.binsh} $out/sh
        ''}"
        "nofail"
      ];
    };
    # We need to bind-mount /bin to /usr/bin, because otherwise upgrading
    # from envfs < 1.0.5 will cause having the old envs with no /bin bind mount.
    # Systemd is smart enough to not mount /bin if it's already mounted.
    "/bin" = {
      device = "/usr/bin";
      options = [ "bind" "nofail" ];
    };
  };
in
{
  services.envfs.enable = false;
  environment.systemPackages = [ (pkgs.callPackage ../. { }) ];
  fileSystems = if config.virtualisation ? qemu then lib.mkVMOverride mounts else mounts;

  system.activationScripts.usrbinenv = lib.mkForce "";
  system.activationScripts.binsh = lib.mkForce "";
}
