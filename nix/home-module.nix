{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.beanKey;
  packages = self.packages.${pkgs.stdenv.hostPlatform.system};
  settings = import ./settings.nix {
    inherit
      cfg
      lib
      pkgs
      packages
      ;
  };
  inputMethod = packages.macos-input-method.withConfig settings.configFile;
in
{
  options.programs.beanKey = settings.options;
  config = lib.mkIf cfg.enable {
    assertions = settings.assertions ++ [
      {
        assertion = pkgs.stdenv.hostPlatform.isDarwin;
        message = "The beanKey Home Manager module requires macOS; use nixosModules.default on NixOS.";
      }
    ];
    home.packages = [ inputMethod ];
    home.activation.beanKeyInputMethod = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
      run ${inputMethod}/bin/beankey-install
    '';
  };
}
