# Evaluate with: import ./nix/tests/quick-tiles.nix { inherit lib; }
# Uses the actual Home Manager module with small fixture options; no builds/services.
{lib}: let
  names = ["polarbar" "permafrostd" "avalaunch" "iceout" "snowconf" "subniri-cli" "icepickd"];
  fixture = {
    type = "derivation";
    name = "fixture";
    outPath = "/fixture";
  };
  self.packages.test = lib.genAttrs names (_: fixture);
  evaluateWithFavorites = menu: favorites:
    lib.evalModules {
      specialArgs.pkgs.stdenv.hostPlatform.system = "test";
      modules = [
        (import ../home-manager/subniri.nix {inherit self;})
        {
          options = {
            home = lib.mkOption {
              type = lib.types.attrs;
              default = {};
            };
            xdg.configHome = lib.mkOption {type = lib.types.str;};
            xdg.configFile = lib.mkOption {
              type = lib.types.attrsOf (lib.types.submodule {options.text = lib.mkOption {type = lib.types.str;};});
              default = {};
            };
            systemd = lib.mkOption {
              type = lib.types.attrs;
              default = {};
            };
            assertions = lib.mkOption {
              type = lib.types.listOf lib.types.attrs;
              default = [];
            };
            warnings = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [];
            };
          };
          config = {
            xdg.configHome = "/fixture";
            services.subniri = {
              enable = true;
              settings.systemMenu = menu;
              settings.fileberg.favorites = favorites;
            };
          };
        }
      ];
    };
  evaluate = menu: evaluateWithFavorites menu [];
  render = menu: (evaluate menu).config.xdg.configFile."subniri/config.kdl".text;
  favorites = (evaluateWithFavorites {} ["copy" "new-folder"]).config.xdg.configFile."subniri/config.kdl".text;
  rows = render {
    rows = [
      [
        {
          widget = "nightlight";
          width = 4;
          height = 2;
        }
      ]
      [
        {gap = 4;}
        {
          widget = "light.office";
          width = 1;
        }
      ]
      []
    ];
  };
  legacy = render {widgets = ["wifi" "script.example"];};
  mixed = evaluate {
    rows = [[]];
    widgets = ["wifi"];
  };
in
  assert lib.hasInfix ''widget "nightlight" height=2 width=4'' rows;
  assert lib.hasInfix ''gap width=4'' rows;
  assert lib.hasInfix ''widget "light.office" width=1'' rows;
  assert lib.hasInfix "    row\n" rows;
  assert lib.hasInfix ''widgets "wifi" "script.example"'' legacy;
  assert lib.any (check: !check.assertion) mixed.config.assertions;
  assert lib.hasInfix ''favorites "copy" "new-folder"'' favorites; {
    passed = 7;
    generated = rows;
  }
