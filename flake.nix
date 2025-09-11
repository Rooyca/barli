{
  description = "barli - a lightweight status bar";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    {
      self,
      nixpkgs,
    }:
      let
        system = "x86_64-linux";
        pkgs = import nixpkgs { inherit system; };

        x11Deps = [
          pkgs.xorg.libX11
        ];
      in
      {
        packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
          pname = "barli";
          version = "0.1.1";

          src = self;
          cargoHash = "sha256-JqLCm48EfhbCcOmQBFsDLyQaoDHMJgXn2/EGzHkShrY=";

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
      };
}
