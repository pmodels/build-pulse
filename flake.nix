{
  description = "build-pulse";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      with import nixpkgs {
        inherit system;
      }; {
        devShell = mkShell {
          packages = [
            openssl
            pkg-config
            python3
            rustup
            sqlite
            uv

            # docs
            texlive.combined.scheme-full
          ];

          env = {
            LD_LIBRARY_PATH = lib.makeLibraryPath [ stdenv.cc.cc.lib ];
            UV_PYTHON = python3.interpreter;
            UV_PYTHON_PREFERENCE = "only-system";
          };
        };
      }
    );
}

# vim: ts=2:sw=2:expandtab:
