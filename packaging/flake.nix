{
  description = "PetWeave Wayland desktop pet framework";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs { inherit system; }));
    in {
      packages = eachSystem (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "petweave";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./../Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.wayland pkgs.libxkbcommon ];
          postInstall = ''
            install -Dm755 target/release/petweave-settings $out/bin/petweave-settings
          '';
        };
      });
    };
}
