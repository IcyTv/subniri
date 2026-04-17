{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.subniri;
  packageSet = self.packages.${pkgs.system};
in {
  options.services.subniri = {
    enable = lib.mkEnableOption "subniri desktop stack";

    package = lib.mkOption {
      type = lib.types.package;
      default = packageSet.subniri-stack;
      description = "Subniri package bundle containing binaries and user units.";
    };

    bar = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Enable the subniri bar service.";
      };
    };

    launcher = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Enable the subniri launcher service.";
      };
    };

    installCli = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Install the subniri CLI into the user profile PATH.";
    };

    cliPackage = lib.mkOption {
      type = lib.types.package;
      default = packageSet.subniri;
      description = "Package that provides the subniri CLI binary.";
    };
  };

  config = lib.mkIf cfg.enable {
    home = {
      packages = lib.mkIf cfg.installCli [cfg.cliPackage];

      file = {
        ".config/systemd/user/subniri.target" = {
          source = "${cfg.package}/share/systemd/user/subniri.target";
        };
        ".config/systemd/user/subniri-bar.service" = lib.mkIf cfg.bar.enable {
          source = "${cfg.package}/share/systemd/user/subniri-bar.service";
        };
        ".config/systemd/user/subniri-launcher.service" = lib.mkIf cfg.launcher.enable {
          source = "${cfg.package}/share/systemd/user/subniri-launcher.service";
        };
      };
    };

    systemd.user.startServices = true;

    home.activation.subniri-systemd-reload = lib.hm.dag.entryAfter ["writeBoundary"] ''
      run ${pkgs.systemd}/bin/systemctl --user daemon-reload
      run ${pkgs.systemd}/bin/systemctl --user enable subniri.target
      run ${pkgs.systemd}/bin/systemctl --user restart subniri.target
    '';
  };
}
