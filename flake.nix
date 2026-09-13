{
  description = "Hypercolor: open-source RGB lighting orchestration engine";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: nixpkgs.legacyPackages.${system};
    in
    {
      overlays.default = final: _prev: {
        hypercolor = final.callPackage ./nix/package.nix { };
      };

      packages = forEachSystem (
        system:
        let
          hypercolor = (pkgsFor system).callPackage ./nix/package.nix { };
        in
        {
          inherit hypercolor;
          default = hypercolor;
        }
      );

      nixosModules = {
        hypercolor =
          { pkgs, ... }:
          {
            imports = [ ./nix/module.nix ];
            services.hypercolor.package =
              nixpkgs.lib.mkDefault
                self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          };
        default = self.nixosModules.hypercolor;
      };

      checks = forEachSystem (
        system:
        let
          # Evaluate the module against a minimal host and build only the
          # user unit, so `nix flake check` proves the options and the unit
          # text without assembling a whole system closure.
          host = nixpkgs.lib.nixosSystem {
            inherit system;
            modules = [
              self.nixosModules.default
              {
                services.hypercolor = {
                  enable = true;
                  input.allDevices = true;
                  extraArgs = [
                    "--log-level"
                    "debug"
                  ];
                };
                fileSystems."/" = {
                  device = "/dev/null";
                  fsType = "ext4";
                };
                boot.loader.grub.enable = false;
                system.stateVersion = "25.05";
              }
            ];
          };
          pkgs = pkgsFor system;
          hypercolor = self.packages.${system}.default;
          unit = host.config.systemd.user.units."hypercolor.service".unit;
        in
        {
          package = hypercolor;
          module =
            assert builtins.elem hypercolor host.config.services.udev.packages;
            assert builtins.elem "i2c-dev" host.config.boot.kernelModules;
            assert builtins.elem "default.target" host.config.systemd.user.services.hypercolor.wantedBy;
            pkgs.runCommand "hypercolor-module-check" { } ''
              unit=${unit}/hypercolor.service
              grep -q -- "--ui-dir ${hypercolor}/share/hypercolor/ui" "$unit"
              grep -q -- "--effects-dir ${hypercolor}/share/hypercolor/effects/bundled" "$unit"
              grep -q -- "--log-level debug" "$unit"
              grep -q "^ProtectSystem=strict" "$unit"
              grep -q "^HYPERCOLOR_LOG=info" "$unit" || grep -q 'HYPERCOLOR_LOG=info' "$unit"
              touch $out
            '';
        }
      );
    };
}
