{
  description = "Development environment for beanKey";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      nixosModules.default = import ./nix/module.nix { inherit self; };

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          assets = import ./nix/assets.nix { inherit pkgs; };
          runtimePackages = import ./nix/packages.nix { inherit assets pkgs; };
        in
        assets
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
          runtimePackages
          // {
            default = runtimePackages.fcitx5-addon;
          }
        )
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          development = import ./nix/dev-shell.nix {
            inherit pkgs;
            dictionary = self.packages.${system}.dictionary;
            emoji = self.packages.${system}.emoji;
            model = self.packages.${system}.model;
            tokenizer = self.packages.${system}.tokenizer;
          };
        in
        {
          default = development.shell;
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          development = import ./nix/dev-shell.nix {
            inherit pkgs;
            dictionary = self.packages.${system}.dictionary;
            emoji = self.packages.${system}.emoji;
            model = self.packages.${system}.model;
            tokenizer = self.packages.${system}.tokenizer;
          };
        in
        import ./nix/checks.nix {
          developmentPackages = development.packages;
          inherit
            nixpkgs
            pkgs
            self
            system
            ;
        }
      );
    };
}
