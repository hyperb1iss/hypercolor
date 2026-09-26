# NixOS module for Hypercolor.
#
#   services.hypercolor.enable = true;
#
# installs the package, the vendor udev rules, the i2c-dev kernel module for
# SMBus discovery, and a hardened systemd user service that starts the daemon
# with every graphical login. The service is a *user* unit on purpose: screen
# capture goes through the XDG desktop portal and host input capture relies
# on logind uaccess ACLs, both of which only exist inside a user session.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.hypercolor;
  inherit (lib)
    mkEnableOption
    mkOption
    mkPackageOption
    mkIf
    mkDefault
    types
    escapeShellArgs
    ;

  allInputRules = pkgs.writeTextDir "lib/udev/rules.d/70-hypercolor-input-all.rules" (
    builtins.readFile ../udev/70-hypercolor-input-all.rules
  );
in
{
  options.services.hypercolor = {
    enable = mkEnableOption "Hypercolor RGB lighting daemon";

    package = mkPackageOption pkgs "hypercolor" { };

    autoStart = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Start the daemon with every graphical login by wanting the user
        service from `default.target`. Disable to keep the unit installed
        but only start it on demand (`systemctl --user start hypercolor`).
      '';
    };

    logLevel = mkOption {
      type = types.enum [
        "error"
        "warn"
        "info"
        "debug"
        "trace"
      ];
      default = "info";
      description = "Value of `HYPERCOLOR_LOG` for the daemon service.";
    };

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [
        "--bind"
        "0.0.0.0:9420"
      ];
      description = "Additional command-line arguments passed to `hypercolor-daemon`.";
    };

    smbus.enable = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Load the `i2c-dev` kernel module so the daemon can discover SMBus
        RGB controllers (motherboard headers, DRAM). The matching device-node
        permissions ship in the package's udev rules.
      '';
    };

    input.allDevices = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Grant the seated user read access to *every* keyboard and mouse
        event node, not only the supported RGB vendors. This lets effects
        react to laptop-internal keyboards, Bluetooth keyboards, and generic
        mice, but it is equivalent to permitting session-wide keylogging by
        any process running as that user. Leave it off unless you have
        weighed that trade-off.
      '';
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    # 99-hypercolor.rules (hidraw, usb, tty, i2c-dev) and the vendor-scoped
    # 70-hypercolor-input.rules ship inside the package.
    services.udev.packages = [ cfg.package ] ++ lib.optional cfg.input.allDevices allInputRules;

    boot.kernelModules = mkIf cfg.smbus.enable [ "i2c-dev" ];

    # ProtectHome=read-only plus ReadWritePaths= needs every listed path to
    # exist before systemd builds the mount namespace, and nothing else
    # creates them on a fresh NixOS login (the packaged unit for deb, rpm and
    # AUR does it in ExecStartPre). User tmpfiles run at session start, ahead
    # of the unit.
    systemd.user.tmpfiles.rules = [
      "d %h/.config/hypercolor 0700 - - -"
      "d %h/.local/share/hypercolor 0700 - - -"
      "d %h/.local/state/hypercolor 0700 - - -"
    ];

    systemd.user.services.hypercolor = {
      description = "Hypercolor RGB Lighting Daemon";
      documentation = [ "https://github.com/hyperb1iss/hypercolor" ];
      after = [
        "graphical-session.target"
        "dbus.socket"
      ];
      wants = [ "graphical-session.target" ];
      wantedBy = mkIf cfg.autoStart [ "default.target" ];

      environment = {
        HYPERCOLOR_LOG = cfg.logLevel;
        RUST_BACKTRACE = "1";
        HYPERCOLOR_SERVICE_IDENTITY = "user_service:systemd:hypercolor.service";
      };

      serviceConfig = {
        Type = "notify";
        ExecStart = escapeShellArgs (
          [
            "${cfg.package}/bin/hypercolor-daemon"
            "--ui-dir"
            "${cfg.package}/share/hypercolor/ui"
            "--effects-dir"
            "${cfg.package}/share/hypercolor/effects/bundled"
          ]
          ++ cfg.extraArgs
        );
        WatchdogSec = 30;
        Restart = "on-failure";
        RestartSec = 3;

        # Same hardening as the packaged unit for deb, rpm, and AUR.
        ProtectHome = "read-only";
        ProtectSystem = "strict";
        ReadWritePaths = [
          "%h/.config/hypercolor"
          "%h/.local/share/hypercolor"
          "%h/.local/state/hypercolor"
        ];
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };

    # Screen-reactive effects on Wayland capture through the desktop portal.
    xdg.portal.enable = mkDefault true;
  };
}
