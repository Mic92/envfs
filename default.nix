{ pkgs ? import <nixpkgs> {}, src ? ./. }:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  inherit src;
  cargoSha256 = "sha256-dcITMNaOpzSnWzICmgdnrYzmDoNynH5ADBECtUTkNvE=";

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
