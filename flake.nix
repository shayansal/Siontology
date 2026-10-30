{
  description = "Siontology: a post-quantum neo-blockchain for agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # nix build; nix run . -- doctor
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "sion";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "sion-cli" "--bin" "sion" ];
          # aws-lc (the cryptography under TLS) is built from its C sources.
          nativeBuildInputs = [ pkgs.cmake pkgs.perl ];
          # The workspace's tests run in its own CI; some need a network.
          doCheck = false;
          meta = {
            description = "Siontology's command line: wallets, a devnet, and agents";
            homepage = "https://siontology.org";
            license = [ pkgs.lib.licenses.asl20 pkgs.lib.licenses.mit ];
            mainProgram = "sion";
          };
        };
      });

      # nix develop: the toolchain, and what the agents' tools use.
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.cmake pkgs.perl ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.bubblewrap pkgs.xvfb-run pkgs.xdotool pkgs.imagemagick ];
        };
      });
    };
}
