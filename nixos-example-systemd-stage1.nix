{
  imports = [ ./modules/envfs.nix ];
  boot.initrd.systemd.enable = true;
}
