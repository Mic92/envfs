{ pkgs ? import <nixpkgs> {}, packageSrc ? ./. }:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  src = packageSrc;

  cargoVendorDir = "vendor";

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
