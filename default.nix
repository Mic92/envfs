{ pkgs ? import <nixpkgs> {}}:
pkgs.rustPlatform.buildRustPackage {
  pname = "envfs";
  version = "0.0.1";
  src = ./.;
  cargoSha256 = "sha256-9R3JD4DqQq/hWzN7wqkVUFKDj4+d4mo3pAfFLt69WxM=";
}
