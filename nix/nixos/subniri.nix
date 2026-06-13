{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.subniri.resumeRestart;
  inherit (lib) mkEnableOption mkIf mkOption types;

  restartUnits = lib.escapeShellArgs cfg.units;
  restartUser = user: ''
    if ! ${pkgs.systemd}/bin/systemctl --machine=${lib.escapeShellArg "${user}@"} --user restart ${restartUnits}; then
      echo "Failed to restart Subniri user units for ${lib.escapeShellArg user}" >&2
    fi
  '';

  restartScript = pkgs.writeShellScript "subniri-resume-restart" ''
    ${lib.concatMapStringsSep "\n" restartUser cfg.users}
  '';
in {
  options.services.subniri.resumeRestart = {
    enable = mkEnableOption "restarting Subniri user services after system resume";

    users = mkOption {
      type = types.listOf types.str;
      default = [];
      example = ["michael"];
      description = "Users whose Subniri user services should be restarted after resume.";
    };

    units = mkOption {
      type = types.listOf types.str;
      default = ["polarbar.service" "avalaunch.service"];
      description = "User systemd units to restart after resume.";
    };

    sleepTargets = mkOption {
      type = types.listOf types.str;
      default = [
        "hibernate.target"
        "hybrid-sleep.target"
        "suspend-then-hibernate.target"
        "suspend.target"
      ];
      description = "System sleep targets that should trigger the resume restart hook.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.users != [];
        message = "services.subniri.resumeRestart.users must contain at least one user.";
      }
      {
        assertion = cfg.units != [];
        message = "services.subniri.resumeRestart.units must contain at least one unit.";
      }
    ];

    systemd.services.subniri-resume-restart = {
      description = "Restart Subniri user services after resume";
      before = cfg.sleepTargets;
      wantedBy = cfg.sleepTargets;

      unitConfig.StopWhenUnneeded = true;

      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${pkgs.coreutils}/bin/true";
        ExecStop = restartScript;
      };
    };
  };
}
