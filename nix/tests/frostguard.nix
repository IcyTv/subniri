# Fixture option evaluation only: does not generate or authenticate real PAM policy.
{lib}: let
  evaluate = settings:
    (lib.evalModules {
      specialArgs.pkgs = {
        systemd = "/fixture/systemd";
        coreutils = "/fixture/coreutils";
        writeShellScript = _: text: text;
      };
      modules = [
        (import ../nixos/subniri.nix {self = {};})
        {
          options = {
            environment.systemPackages = lib.mkOption {
              type = lib.types.listOf lib.types.package;
              default = [];
            };
            security.pam.services = lib.mkOption {
              type = lib.types.attrsOf (lib.types.submodule {
                options = {
                  enable = lib.mkOption {
                    type = lib.types.bool;
                    default = true;
                  };
                  text = lib.mkOption {
                    type = lib.types.lines;
                    default = "native-generated-policy-fixture";
                  };
                };
              });
              default = {};
            };
            warnings = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [];
            };
            systemd.services = lib.mkOption {
              type = lib.types.attrs;
              default = {};
            };
            assertions = lib.mkOption {
              type = lib.types.listOf lib.types.attrs;
              default = [];
            };
          };
        }
        settings
      ];
    }).config;
  enabled = evaluate {};
  disabled = evaluate {security.pam.services.frostguard.enable = false;};
  overridden = evaluate {security.pam.services.frostguard.text = "administrator policy\n";};
  resume = evaluate {
    services.subniri.resumeRestart = {
      enable = true;
      users = ["fixture"];
    };
  };
  resumeDisabledPam = evaluate {
    security.pam.services.frostguard.enable = false;
    services.subniri.resumeRestart = {
      enable = true;
      users = ["fixture"];
    };
  };
in
  assert enabled.security.pam.services.frostguard.enable;
  assert enabled.security.pam.services.frostguard.text == "native-generated-policy-fixture";
  assert enabled.environment.systemPackages == [];
  assert enabled.warnings == [];
  assert enabled.systemd.services == {};
  assert !enabled.services.subniri.resumeRestart.enable;
  assert !disabled.security.pam.services.frostguard.enable;
  assert disabled.environment.systemPackages == [];
  assert overridden.security.pam.services.frostguard.text == "administrator policy\n";
  assert resume.environment.systemPackages == [];
  assert resume.security.pam.services.frostguard.enable;
  assert resume.systemd.services ? subniri-resume-restart;
  assert resumeDisabledPam.systemd.services.subniri-resume-restart == resume.systemd.services.subniri-resume-restart; {
    passed = 13;
  }
