{ pkgs ? import <nixpkgs> {}, src ? ./. }:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  inherit src;

  cargoVendorDir = "vendor";

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
