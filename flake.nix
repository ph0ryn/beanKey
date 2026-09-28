{
  description = "Development environment for beanKey";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      home-manager,
      ...
    }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
      developmentFor =
        system:
        import ./nix/dev-shell.nix {
          pkgs = pkgsFor system;
          inherit (self.packages.${system})
            dictionary
            emoji
            model
            tokenizer
            ;
        };
    in
    {
      nixosModules.default = import ./nix/module.nix { inherit self; };
      homeModules.default = import ./nix/home-module.nix { inherit self; };

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          assets = import ./nix/assets.nix { inherit pkgs; };
          runtimePackages = import ./nix/packages.nix { inherit assets pkgs; };
          publishedPackages = assets // runtimePackages;
        in
        publishedPackages
        // {
          default = pkgs.linkFarm "beankey-${runtimePackages.daemon.version}" (
            pkgs.lib.mapAttrsToList (name: path: { inherit name path; }) publishedPackages
          );
        }
      );

      devShells = forAllSystems (system: {
        default = (developmentFor system).shell;
      });

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      checks = forAllSystems (
        system:
        import ./nix/checks.nix {
          pkgs = pkgsFor system;
          developmentPackages = (developmentFor system).packages;
          inherit
            home-manager
            nixpkgs
            self
            system
            ;
        }
      );
    };
}
