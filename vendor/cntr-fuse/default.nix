with import <nixpkgs> {};
stdenv.mkDerivation {
  name = "env";
  buildInputs = [
    bashInteractive
    cargo
    rustc
    pkg-config
    fuse
  ];
}
