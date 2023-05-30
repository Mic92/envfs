{ pkgs ? import <nixpkgs> { }, packageSrc ? ./. }:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "1.0.1";
  src = packageSrc;

  cargoLock.lockFile = ./Cargo.lock;

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
