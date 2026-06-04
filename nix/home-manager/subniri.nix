{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.subniri;
in {
  options.services.subniri = {
    enable = lib.mkEnableOption "Enable Subniri desktop shell";

    polarbar = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to enable the Polarbar systemd service.";
      };

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.polarbar;
        description = "The Polarbar package to use.";
      };
    };

    permafrostd = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to enable the Permafrostd service.";
      };

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.permafrostd;
        description = "The Permafrostd package to use.";
      };
    };

    avalaunch = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to enable the Avalaunch application launcher.";
      };

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.avalaunch;
        description = "The Avalaunch package to use.";
      };
    };

    iceout = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to install the iceout (logout) application.";
      };
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.iceout;
        description = "The Iceout package to use.";
      };
    };

    snowconf = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to enable the Snowconf settings application.";
      };
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.snowconf;
        description = "The Snowconf package to use.";
      };
    };

    cli = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to enable the Subniri CLI tool.";
      };
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.subniri-cli;
        description = "The Subniri CLI package to use.";
      };
    };

    systemd.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to enable systemd services for Subniri components.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages =
      lib.optionals cfg.polarbar.enable [cfg.polarbar.package]
      ++ lib.optionals cfg.permafrostd.enable [cfg.permafrostd.package]
      ++ lib.optionals cfg.avalaunch.enable [cfg.avalaunch.package]
      ++ lib.optionals cfg.iceout.enable [cfg.iceout.package]
      ++ lib.optionals cfg.snowconf.enable [cfg.snowconf.package]
      ++ lib.optionals cfg.cli.enable [cfg.cli.package];

    systemd.user.services = lib.mkIf cfg.systemd.enable {
      polarbar = lib.mkIf cfg.polarbar.enable {
        Unit = {
          Description = "Polarbar systemd service";
          After = ["graphical-session.target"];
          PartOf = ["graphical-session.target"];
        };

        Service = {
          ExecStart = "${cfg.polarbar.package}/bin/polarbar";
          Restart = "on-failure";
          RestartSec = 5;
        };

        Install = {
          WantedBy = ["graphical-session.target"];
        };
      };

      permafrostd = lib.mkIf cfg.permafrostd.enable {
        Unit = {
          Description = "Permafrostd systemd service";
          After = ["graphical-session.target"];
          PartOf = ["graphical-session.target"];
        };
        Service = {
          ExecStart = "${cfg.permafrostd.package}/bin/permafrostd";
          Restart = "on-failure";
          RestartSec = 5;
        };
        Install = {
          WantedBy = ["graphical-session.target"];
        };
      };

      avalaunch = lib.mkIf cfg.avalaunch.enable {
        Unit = {
          Description = "Avalaunch systemd service";
          After = ["graphical-session.target"];
          PartOf = ["graphical-session.target"];
        };
        Service = {
          ExecStart = "${cfg.avalaunch.package}/bin/avalaunch";
          Restart = "on-failure";
          RestartSec = 5;
        };
        Install = {
          WantedBy = ["graphical-session.target"];
        };
      };
    };
  };
}
