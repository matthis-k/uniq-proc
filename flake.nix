{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-24.05";
  };
  outputs = {
    nixpkgs,
    self,
    ...
  }: {
    packages = {
      "x86_64-linux" = let
        pkgs = nixpkgs.legacyPackages."x86_64-linux";
      in rec {
        uniq-proc = pkgs.rustPlatform.buildRustPackage {
          pname = "uniq-proc";
          version = "0.1.0";

          src = ./.;
          cargoLock = {lockFile = ./Cargo.lock;};

          meta = {
            description = "Tool to manage a few defined processes.";
            license = pkgs.lib.licenses.unlicense;
            maintainers = [];
          };
        };
        default = uniq-proc;
      };
    };
    overlays = {
      uniq-proc = self.outputs.packages."x86_64-linux".uniq-proc;
    };
  };
}
