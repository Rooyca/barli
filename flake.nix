{
  description = "barli - a lightweight status bar";

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
      let
        pkgs = import nixpkgs { inherit system; };

        x11Deps = [
          pkgs.xorg.libX11
        ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "barli";
          version = "0.1.0";

          src = self;
          cargoHash = "sha256-3Oah+p3NV+JCdgNd+FzaEA2LzU0M5Z/zY76np6NHCxY=";

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = x11Deps;

        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = x11Deps ++ [
            pkgs.rustc
            pkgs.cargo
          ];
        };
      }
    );
}
