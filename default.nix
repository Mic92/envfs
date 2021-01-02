{ pkgs ? import <nixpkgs> {}}:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  src = ./.;
  cargoSha256 = "sha256-Fma8r1WwyvUmM+BHMLqHRaI5xJhh9cBA9r7YXifwJjU=";

  postInstall = ''
    ln -s envfs $out/bin/mount.envfs
    ln -s envfs $out/bin/mount.fuse.envfs
  '';
}
