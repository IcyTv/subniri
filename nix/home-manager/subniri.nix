{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  inherit (import ../kdl.nix {inherit lib;}) node plain leaf flag serialize;

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
    icepickd = {
      package = packages.icepickd;
      description = "File indexing service";
      bin = "icepickd";
      systemd = true;
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

  systemdComponentUnits = map (name: "${name}.service") (builtins.attrNames systemdComponents);

  mkService = name: component: {
    Unit = {
      Description = componentDefinitions.${name}.description;
      PartOf = ["subniri.target"];
      After = ["graphical-session.target"];
    };

    Service = {
      ExecStart = "${component.package}/bin/${componentDefinitions.${name}.bin}";
      Environment =
        lib.optionals cfg.config.enable [
          "SUBNIRI_CONFIG_FILE=${configPath}"
        ]
        ++ ["RUST_LOG=info"];
      Restart = "on-failure";
      RestartSec = 5;
    };
  };

  subniriTarget = {
    Unit = {
      Description = "Subniri desktop shell";
      Wants = systemdComponentUnits;
      After = ["graphical-session.target"] ++ systemdComponentUnits;
      PartOf = ["graphical-session.target"];
    };

    Install = {
      WantedBy = ["graphical-session.target"];
    };
  };

  renderConfig = cfg: let
    inherit (cfg) settings;
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
    serialize.nodes (
      [
        (plain "nightlight" (
          lib.optionals settings.nightlight.enable [
            (flag "enabled")
          ]
          ++ lib.optionals settings.nightlight.useLocation [
            (flag "use_location")
          ]
          ++ [
            (leaf "debounce_ms" settings.nightlight.debounceMs)
          ]
          ++ lib.optionals (dawn != null) [
            (leaf "dawn" dawn)
          ]
          ++ lib.optionals (dusk != null) [
            (leaf "dusk" dusk)
          ]
          ++ [
            (plain "day" [
              (leaf "temperature" settings.nightlight.day.temperature)
              (leaf "brightness" settings.nightlight.day.brightness)
            ])
            (plain "night" [
              (leaf "temperature" settings.nightlight.night.temperature)
              (leaf "brightness" settings.nightlight.night.brightness)
            ])
          ]
        ))
      ]
      ++ (lib.optionals settings.homeassistant.enable (plain "homeassistant" (
        [(flag "enabled")]
        ++ lib.optionals (settings.homeassistant.url != null) [
          (leaf "url" settings.homeassistant.url)
        ]
        ++ lib.optionals (settings.homeassistant.trackedDevices != []) [
          (node "tracked_devices" settings.homeassistant.trackedDevices [])
        ]
      )))
      ++ (lib.optionals settings.spotify.enable (plain "spotify" [
        (flag "enabled")
      ]))
      ++ [
        (plain "system_menu" (
          lib.optionals (settings.systemMenu.widgets != []) [
            (node "widgets" settings.systemMenu.widgets [])
          ]
        ))
      ]
      ++ lib.optionals cfg.avalaunch.enable [
        (plain "launcher" (
          lib.optionals (settings.launcher.providers != []) [
            (node "providers" settings.launcher.providers [])
          ]
          ++ [
            (plain "fuzzy_search" [
              (leaf "min_chars" settings.launcher.fuzzy_search.min_chars)
              (leaf "short_query_chars" settings.launcher.fuzzy_search.short.chars)
              (leaf "short_max_distance" settings.launcher.fuzzy_search.short.distance)
              (leaf "medium_query_chars" settings.launcher.fuzzy_search.medium.chars)
              (leaf "medium_max_distance" settings.launcher.fuzzy_search.medium.distance)
              (leaf "long_max_distance" settings.launcher.fuzzy_search.long.distance)
            ])
          ]
        ))
      ]
      ++ lib.optionals cfg.icepickd.enable [
        (plain "indexing" [
          (flag "enabled")
        ])
      ]
    );

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

        launcher = {
          providers = mkOption {
            type = types.listOf (types.enum [
              "calculator"
              "applications"
              "files"
            ]);
            default = ["calculator" "applications" "files"];
            description = "Avalaunch providers to enable.";
          };

          fuzzy_search = let
            queryRange = chars: distance: {
              chars = mkOption {
                type = types.ints.between 1 64;
                default = chars;
                description = "Maximum number of characters in the fuzzy search query.";
              };
              distance = mkOption {
                type = types.ints.between 1 8;
                default = distance;
                description = "Maximum levenshtein distance for fuzzy search matches.";
              };
            };
          in {
            min_chars = mkOption {
              type = types.ints.between 1 32;
              default = 3;
              description = "Minimum number of characters to start fuzzy search.";
            };
            short = queryRange 4 1;
            medium = queryRange 7 2;
            long.distance = mkOption {
              type = types.ints.between 1 8;
              default = 3;
              description = "Maximum levenshtein distance for long fuzzy search matches.";
            };
          };
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
      text = renderConfig cfg;
    };

    systemd.user = mkIf (cfg.systemd.enable && systemdComponents != {}) {
      services = lib.mapAttrs mkService systemdComponents;
      targets.subniri = subniriTarget;
    };
  };
}
