{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.subniri;
  inherit (lib) mkEnableOption mkIf mkOption types;

  system = pkgs.stdenv.hostPlatform.system;
  packages = self.packages.${system};

  configPath = "${config.xdg.configHome}/${cfg.config.target}";

  componentDefinitions = {
    polarbar = {
      package = packages.polarbar;
      description = "Polarbar systemd service";
      bin = "polarbar";
      systemd = true;
    };
    permafrostd = {
      package = packages.permafrostd;
      description = "Permafrostd service";
      bin = "permafrostd";
      systemd = true;
    };
    avalaunch = {
      package = packages.avalaunch;
      description = "Avalaunch application launcher";
      bin = "avalaunch";
      systemd = true;
    };
    iceout = {
      package = packages.iceout;
      description = "iceout (logout) application";
    };
    snowconf = {
      package = packages.snowconf;
      description = "Snowconf settings application";
    };
    cli = {
      package = packages.subniri-cli;
      description = "Subniri CLI tool";
    };
  };

  mkComponentOption = name: component: {
    enable = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to enable the ${component.description}.";
    };

    package = mkOption {
      type = types.package;
      default = component.package;
      description = "The ${name} package to use.";
    };
  };

  configuredComponents = lib.genAttrs (builtins.attrNames componentDefinitions) (name: cfg.${name});
  enabledComponents = lib.filterAttrs (_: component: component.enable) configuredComponents;

  systemdComponents =
    lib.filterAttrs (
      name: component:
        componentDefinitions.${name}.systemd or false
        && component.enable
    )
    enabledComponents;

  mkService = name: component: {
    Unit = {
      Description = componentDefinitions.${name}.description;
      After = ["graphical-session.target"];
      PartOf = ["graphical-session.target"];
    };

    Service = {
      ExecStart = "${component.package}/bin/${componentDefinitions.${name}.bin}";
      Environment = lib.optionals cfg.config.enable [
        "SUBNIRI_CONFIG_FILE=${configPath}"
      ];
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = ["graphical-session.target"];
    };
  };

  quote = builtins.toJSON;
  bool = value:
    if value
    then "true"
    else "false";
  nullableString = value:
    if value == null
    then "null"
    else quote value;

  indent = text:
    lib.concatMapStringsSep "\n" (line: "  ${line}")
    (lib.splitString "\n" (lib.removeSuffix "\n" text));

  renderBlock = name: lines: "${name} {\n${indent (lib.concatStringsSep "\n" lines)}\n}";

  renderNightlightSetting = name: value:
    renderBlock name [
      "temperature ${toString value.temperature}"
      "brightness ${toString value.brightness}"
    ];

  renderStringList = name: values:
    if values == []
    then "${name}"
    else renderBlock name (map (value: ''- ${quote value}'') values);

  renderConfig = settings: let
    dawn =
      if settings.nightlight.useLocation
      then null
      else if settings.nightlight.dawn == null
      then "07:00"
      else settings.nightlight.dawn;
    dusk =
      if settings.nightlight.useLocation
      then null
      else if settings.nightlight.dusk == null
      then "20:00"
      else settings.nightlight.dusk;
  in
    lib.concatStringsSep "\n" [
      (renderBlock "nightlight" [
        "enabled ${bool settings.nightlight.enable}"
        "use_location ${bool settings.nightlight.useLocation}"
        "dawn ${nullableString dawn}"
        "dusk ${nullableString dusk}"
        (renderNightlightSetting "day" settings.nightlight.day)
        (renderNightlightSetting "night" settings.nightlight.night)
        "debounce_ms ${toString settings.nightlight.debounceMs}"
      ])
      (renderBlock "homeassistant" [
        "enabled ${bool settings.homeassistant.enable}"
        "url ${nullableString settings.homeassistant.url}"
        (renderStringList "tracked_devices" settings.homeassistant.trackedDevices)
      ])
      (renderBlock "spotify" [
        "enabled ${bool settings.spotify.enable}"
      ])
      (renderBlock "system_menu" [
        (renderStringList "widgets" settings.systemMenu.widgets)
      ])
    ];

  nightlightSettingType = types.submodule {
    options = {
      temperature = mkOption {
        type = types.ints.between 1000 10000;
        description = "Color temperature in Kelvin.";
      };

      brightness = mkOption {
        type = types.float;
        description = "Screen brightness multiplier, between 0.1 and 1.0.";
      };
    };
  };
in {
  options.services.subniri =
    (lib.mapAttrs mkComponentOption componentDefinitions)
    // {
      enable = mkEnableOption "Subniri desktop shell";

      systemd.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to enable systemd services for Subniri components.";
      };

      config = {
        enable = mkOption {
          type = types.bool;
          default = true;
          description = "Whether Home Manager should generate Subniri's KDL config file.";
        };

        target = mkOption {
          type = types.str;
          default = "subniri/config.kdl";
          description = "Path below XDG_CONFIG_HOME for the generated Subniri config file.";
        };
      };

      settings = {
        nightlight = {
          enable = mkOption {
            type = types.bool;
            default = false;
            description = "Whether to enable the nightlight integration.";
          };

          useLocation = mkOption {
            type = types.bool;
            default = false;
            description = "Whether to use location data for dawn and dusk.";
          };

          dawn = mkOption {
            type = types.nullOr types.str;
            default = null;
            example = "07:00";
            description = "Time when nightlight switches to day settings. Defaults to 07:00 when useLocation is false.";
          };

          dusk = mkOption {
            type = types.nullOr types.str;
            default = null;
            example = "20:00";
            description = "Time when nightlight switches to night settings. Defaults to 20:00 when useLocation is false.";
          };

          day = mkOption {
            type = nightlightSettingType;
            default = {
              temperature = 6500;
              brightness = 1.0;
            };
            description = "Daytime nightlight settings.";
          };

          night = mkOption {
            type = nightlightSettingType;
            default = {
              temperature = 2500;
              brightness = 0.7;
            };
            description = "Nighttime nightlight settings.";
          };

          debounceMs = mkOption {
            type = types.ints.between 0 10000;
            default = 500;
            description = "Debounce delay for gamma table changes, in milliseconds.";
          };
        };

        homeassistant = {
          enable = mkOption {
            type = types.bool;
            default = false;
            description = "Whether to enable the Home Assistant integration.";
          };

          url = mkOption {
            type = types.nullOr types.str;
            default = null;
            example = "http://homeassistant.local:8123";
            description = "URL of the Home Assistant instance.";
          };

          trackedDevices = mkOption {
            type = types.listOf types.str;
            default = [];
            description = "Home Assistant device IDs to expose in Subniri.";
          };
        };

        spotify.enable = mkOption {
          type = types.bool;
          default = false;
          description = "Whether to enable the Spotify integration.";
        };

        systemMenu.widgets = mkOption {
          type = types.listOf (types.enum [
            "Wifi"
            "Bluetooth"
            "Speaker"
            "Microphone"
            "Vpn"
            "Nightlight"
          ]);
          default = [];
          description = "Widgets to display in the system menu.";
        };
      };
    };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.settings.nightlight.useLocation -> cfg.settings.nightlight.dawn == null;
        message = "services.subniri.settings.nightlight.dawn must be null when useLocation is true.";
      }
      {
        assertion = cfg.settings.nightlight.useLocation -> cfg.settings.nightlight.dusk == null;
        message = "services.subniri.settings.nightlight.dusk must be null when useLocation is true.";
      }
      {
        assertion = cfg.settings.nightlight.day.brightness >= 0.1 && cfg.settings.nightlight.day.brightness <= 1.0;
        message = "services.subniri.settings.nightlight.day.brightness must be between 0.1 and 1.0.";
      }
      {
        assertion = cfg.settings.nightlight.night.brightness >= 0.1 && cfg.settings.nightlight.night.brightness <= 1.0;
        message = "services.subniri.settings.nightlight.night.brightness must be between 0.1 and 1.0.";
      }
    ];

    home.packages = map (component: component.package) (lib.attrValues enabledComponents);

    home.sessionVariables = mkIf cfg.config.enable {
      SUBNIRI_CONFIG_FILE = configPath;
    };

    xdg.configFile.${cfg.config.target} = mkIf cfg.config.enable {
      text = renderConfig cfg.settings;
    };

    systemd.user.services = mkIf cfg.systemd.enable (lib.mapAttrs mkService systemdComponents);
  };
}
