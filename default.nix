{ pkgs ? import <nixpkgs> {}}:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  src = ./.;
  cargoSha256 = "sha256-sxHYUkXf5OIAIZG2VKmTg9qCuDi146Uhzxdv3wF4Fzw=";

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
