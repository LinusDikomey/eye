{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };
  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    overlays = [(import rust-overlay)];
    supportedSystems = [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-darwin"
      "x86_64-linux"
    ];
    forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    pkgsFor = system: import nixpkgs {inherit system overlays;};
    rustToolchain = pkgs: pkgs.rust-bin.stable.latest.default;
    rustPlatform = pkgs:
      pkgs.makeRustPlatform {
        cargo = rustToolchain pkgs;
        rustc = rustToolchain pkgs;
      };
  in {
    packages = forAllSystems (
      system: let
        pkgs = pkgsFor system;
      in {
        default = (rustPlatform pkgs).buildRustPackage {
          pname = "eye";
          version = "0.1";
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "color-format-0.1.0" = "sha256-ErorDyhDtycDgDb8tyQe6TQ+lz552azJ4G1V5bhOb/E=";
            };
          };
          buildFeatures = ["lsp"];
          src = pkgs.lib.cleanSource ./.;
          nativeBuildInputs = with pkgs; [
            llvmPackages_21.llvm
            libxml2
          ];
          env.EYE_DEFAULT_STD = "${placeholder "out"}/lib/std";
          postInstall = ''
            mkdir -p $out/lib
            cp -r std $out/lib
          '';
        };
      }
    );
    devShells = forAllSystems (
      system: let
        pkgs = pkgsFor system;
      in {
        default = pkgs.mkShell {
          inputsFrom = [self.packages.${system}.default];
          nativeBuildInputs = [
            ((rustToolchain pkgs).override {
              extensions = [
                "rust-analyzer"
                "rust-src"
              ];
            })
          ];
          RUST_BACKTRACE = 1;
        };
      }
    );
  };
}
