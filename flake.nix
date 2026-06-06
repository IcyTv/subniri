{
  description = "Build a cargo workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    cargo2nix = {
      url = "github:cargo2nix/cargo2nix";
      inputs.rust-overlay.follows = "rust-overlay";
    };

    nvim = {
      url = "github:IcyTv/nvim.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  nixConfig = {
    extra-substituters = ["https://icytv.cachix.org"];
    extra-trusted-public-keys = ["icytv.cachix.org-1:epXlDqA5apfoHPIc+Z7Vx6aPN7Tsz2hzik62V5Rs5sQ="];
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    nvim,
    cargo2nix,
    ...
  }: let
    overlay = final: _prev: let
      inherit (final.stdenv.hostPlatform) system;
    in {
      inherit (self.packages.${system}) subniri-shell;
    };

    systems = flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay) cargo2nix.overlays.default];
          config.allowUnfreePredicate = pkg:
            builtins.elem (nixpkgs.lib.getName pkg) [
              "cmp-spell"
            ];
        };

        inherit (pkgs) lib;

        rustToolchain = pkgs.rust-bin.stable.latest.minimal;
        rustLib = pkgs.rustBuilder.rustLib;

        mkOverride = args: rustLib.makeOverride args;
        appendList = attr: values: drv: (drv.${attr} or []) ++ values;
        appendScript = attr: script: drv: (drv.${attr} or "") + script;

        cargoPatch = path: replacements: let
          replaceArg = {
            from,
            to,
          }: "--replace ${lib.escapeShellArg from} ${lib.escapeShellArg to}";
          replaceArgs = lib.concatMapStringsSep " \\\n            " replaceArg replacements;
        in "substituteInPlace ${path} \\\n            ${replaceArgs}\n";

        winitWorkspaceMetadata = [
          {
            from = "rust-version.workspace = true";
            to = ''rust-version = "1.70.0"'';
          }
          {
            from = "repository.workspace = true";
            to = ''repository = "https://github.com/rust-windowing/winit"'';
          }
          {
            from = "license.workspace = true";
            to = ''license = "Apache-2.0"'';
          }
          {
            from = "edition.workspace = true";
            to = ''edition = "2021"'';
          }
          {
            from = "serde = { workspace = true, optional = true }";
            to = ''serde = { version = "1", features = ["serde_derive"], optional = true }'';
          }
        ];

        dpiWorkspaceMetadata =
          winitWorkspaceMetadata
          ++ [
            {
              from = "mint = { workspace = true, optional = true }";
              to = ''mint = { version = "0.5.6", optional = true }'';
            }
          ];

        icedWorkspaceMetadata = [
          {
            from = "rust-version.workspace = true";
            to = ''rust-version = "1.92"'';
          }
          {
            from = "version.workspace = true";
            to = ''version = "0.15.0-dev"'';
          }
          {
            from = "edition.workspace = true";
            to = ''edition = "2024"'';
          }
          {
            from = "authors.workspace = true";
            to = "authors = []";
          }
          {
            from = "license.workspace = true";
            to = ''license = "MIT"'';
          }
          {
            from = "repository.workspace = true";
            to = ''repository = "https://github.com/iced-rs/iced"'';
          }
          {
            from = "homepage.workspace = true";
            to = ''homepage = "https://iced.rs"'';
          }
          {
            from = "categories.workspace = true";
            to = ''categories = ["gui"]'';
          }
          {
            from = "keywords.workspace = true";
            to = ''keywords = ["gui", "ui", "graphics", "interface", "widgets"]'';
          }
          {
            from = "iced_debug.workspace = true";
            to = ''iced_debug = { version = "0.15.0-dev", path = "debug" }'';
          }
          {
            from = "iced_core.workspace = true";
            to = ''iced_core = { version = "0.15.0-dev", path = "core" }'';
          }
          {
            from = "iced_futures.workspace = true";
            to = ''iced_futures = { version = "0.15.0-dev", path = "futures" }'';
          }
          {
            from = "iced_renderer.workspace = true";
            to = ''iced_renderer = { version = "0.15.0-dev", path = "renderer" }'';
          }
          {
            from = "iced_runtime.workspace = true";
            to = ''iced_runtime = { version = "0.15.0-dev", path = "runtime" }'';
          }
          {
            from = "iced_widget.workspace = true";
            to = ''iced_widget = { version = "0.15.0-dev", path = "widget" }'';
          }
          {
            from = "iced_winit.workspace = true";
            to = ''iced_winit = { version = "0.15.0-dev", path = "winit" }'';
          }
          {
            from = "thiserror.workspace = true";
            to = ''thiserror = "2"'';
          }
        ];

        icedOptionalDeps = [
          {
            from = "iced_devtools.workspace = true";
            to = ''iced_devtools = { version = "0.15.0-dev", path = "devtools", optional = true }'';
          }
          {
            from = "iced_devtools.optional = true";
            to = "# optional moved into inline table";
          }
          {
            from = "iced_tester.workspace = true";
            to = ''iced_tester = { version = "0.15.0-dev", path = "tester", optional = true }'';
          }
          {
            from = "iced_tester.optional = true";
            to = "# optional moved into inline table";
          }
          {
            from = "iced_highlighter.workspace = true";
            to = ''iced_highlighter = { version = "0.15.0-dev", path = "highlighter", optional = true }'';
          }
          {
            from = "iced_highlighter.optional = true";
            to = "# optional moved into inline table";
          }
          {
            from = "image.workspace = true";
            to = ''image = { version = "0.25", default-features = false, optional = true }'';
          }
          {
            from = "image.optional = true";
            to = "# optional moved into inline table";
          }
          {
            from = "iced_wgpu.workspace = true";
            to = ''iced_wgpu = { version = "0.15.0-dev", path = "wgpu" }'';
          }
          {
            from = "workspace = true";
            to = "# removed workspace lint inheritance for cargo2nix";
          }
        ];

        waylandRuntimeLibs = with pkgs; [wayland libxkbcommon vulkan-loader];
        guiBuildInputs = with pkgs; [wayland libxkbcommon];

        mkWrappedGuiOverride = {
          name,
          binName ? name,
          buildInputs ? guiBuildInputs,
          runtimeLibs ? waylandRuntimeLibs,
          wrapArgs ? [],
          extraAttrs ? {},
        }: let
          wrapperArgs =
            [
              "--prefix"
              "LD_LIBRARY_PATH"
              ":"
              (lib.makeLibraryPath runtimeLibs)
              "--set"
              "XKB_CONFIG_ROOT"
              "${pkgs.xkeyboard_config}/share/X11/xkb"
            ]
            ++ wrapArgs;
          wrapperArgLines = lib.concatMapStringsSep "\n" (arg: "  ${lib.escapeShellArg arg}") wrapperArgs;
        in
          mkOverride {
            inherit name;

            overrideAttrs = drv:
              {
                nativeBuildInputs = appendList "nativeBuildInputs" [pkgs.makeWrapper] drv;
                buildInputs = appendList "buildInputs" buildInputs drv;

                postInstall =
                  appendScript "postInstall" "makeWrapperArgs=(\n${wrapperArgLines}\n)\nwrapProgram $bin/bin/${binName} \"\${makeWrapperArgs[@]}\"\n"
                  drv;
              }
              // extraAttrs;
          };

        workspaceSrc = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./crates
            ./assets
          ];
        };

        rustPkgs = pkgs.rustBuilder.makePackageSet {
          packageFun = import ./Cargo.nix;

          inherit rustToolchain workspaceSrc;

          packageOverrides = pkgs:
            pkgs.rustBuilder.overrides.all
            ++ [
              (mkOverride {
                registry = "unknown";

                overrideArgs = old: {
                  profile =
                    if old.profile == null
                    then null
                    else old.profile // {lto = false;};
                };

                overrideAttrs = drv: {
                  postPatch =
                    (drv.postPatch or "")
                    + ''
                      substituteInPlace Cargo.toml \
                        --replace 'lto = true' 'lto = false'
                    '';
                };
              })
              (mkOverride {
                name = "smithay-client-toolkit";
                overrideAttrs = drv: {
                  nativeBuildInputs = appendList "nativeBuildInputs" [pkgs.pkg-config] drv;
                  buildInputs = appendList "buildInputs" [pkgs.libxkbcommon] drv;
                };
              })
              (mkOverride {
                name = "libpulse-sys";

                overrideAttrs = drv: {
                  propagatedBuildInputs = appendList "propagatedBuildInputs" [pkgs.libpulseaudio] drv;
                  nativeBuildInputs = appendList "nativeBuildInputs" [pkgs.pkg-config] drv;
                  postInstall =
                    appendScript "postInstall" ''
                      rm -f $out/lib/.link-flags
                    ''
                    drv;
                };
              })
              (mkOverride {
                registry = "git+https://github.com/iced-rs/winit.git";
                name = "dpi";

                overrideAttrs = drv: {
                  postPatch = appendScript "postPatch" (cargoPatch "Cargo.toml" dpiWorkspaceMetadata) drv;
                };
              })
              (mkOverride {
                registry = "git+https://github.com/iced-rs/winit.git";
                name = "winit";

                overrideAttrs = drv: {
                  postPatch =
                    appendScript "postPatch" ''
                      ${cargoPatch "Cargo.toml" winitWorkspaceMetadata}
                      ${cargoPatch "dpi/Cargo.toml" dpiWorkspaceMetadata}
                    ''
                    drv;
                };
              })
              (mkOverride {
                registry = "git+https://github.com/IcyTv/iced";
                name = "iced";

                overrideAttrs = drv: {
                  postPatch = appendScript "postPatch" (cargoPatch "Cargo.toml" (icedWorkspaceMetadata ++ icedOptionalDeps)) drv;
                };
              })
              (mkOverride {
                registry = "unknown";

                overrideAttrs = _drv: {
                  PHOSPHOR_ICONS = "${phosphorIcons}";
                };
              })
              (mkWrappedGuiOverride {
                name = "polarbar";
                buildInputs = guiBuildInputs ++ [pkgs.pipewire];
                runtimeLibs = waylandRuntimeLibs ++ [pkgs.pipewire];
                wrapArgs = [
                  "--set"
                  "SPA_PLUGIN_DIR"
                  "${pkgs.pipewire}/lib/spa-0.2"
                ];
                extraAttrs = {
                  SUBNIRI_ICEOUT_BIN = "${rustPkgs.workspace.iceout {}}/bin/iceout";
                  SUBNIRI_SNOWCONF_BIN = "${rustPkgs.workspace.settings {}}/bin/snowconf";
                };
              })
              (mkWrappedGuiOverride {
                name = "iceout";
              })
              (mkWrappedGuiOverride {
                name = "settings";
                binName = "snowconf";
              })
            ];
        };

        # Common arguments can be set here to avoid repeating them later
        # Note: changes here will rebuild all dependency crates
        commonArgs = {
          strictDeps = true;
          RUSTFLAGS = "-C link-arg=-fuse-ld=lld";

          buildInputs = with pkgs; [
            fontconfig
            wayland
            libxkbcommon
            dbus
            pam
            vulkan-headers
            pipewire
            libpulseaudio
          ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          nativeBuildInputs = with pkgs; [
            pkg-config
            rustPlatform.bindgenHook
            rustToolchain
          ];
        };

        phosphorIcons = pkgs.stdenv.mkDerivation {
          name = "phosphor-icons";

          src = pkgs.fetchzip {
            url = "https://phosphoricons.com/assets/phosphor-icons.zip";
            sha256 = "sha256-vrYPR6pzPO6b8lQI8/kY1pT8txj6KDampIg3qcgiAL8=";
            stripRoot = false;
          };

          installPhase = ''
            mkdir -p $out
            cp -r SVGs\ Flat/* $out/
          '';
        };

        workspacePackageList = [
          {
            name = "subniri-cli";
            package = rustPkgs.workspace.cli {};
          }
          {
            name = "polarbar";
            package = rustPkgs.workspace.polarbar {};
          }
          {
            name = "permafrostd";
            package = rustPkgs.workspace.daemon {};
          }
          {
            name = "avalaunch";
            package = rustPkgs.workspace.launcher {};
          }
          {
            name = "iceout";
            package = rustPkgs.workspace.iceout {};
          }
          {
            name = "snowconf";
            package = rustPkgs.workspace.settings {};
          }
        ];

        workspacePackages =
          builtins.listToAttrs
          (map ({
            name,
            package,
          }:
            lib.nameValuePair name package)
          workspacePackageList);
      in {
        packages =
          workspacePackages
          // {
            default = pkgs.symlinkJoin {
              name = "subniri";

              paths = map ({package, ...}: package) workspacePackageList;
            };
          };

        apps = {
        };

        devShells.default = pkgs.mkShell {
          inherit (commonArgs) RUSTFLAGS;

          LD_LIBRARY_PATH = lib.makeLibraryPath (with pkgs; [
            wayland
            libxkbcommon
            fontconfig
            vulkan-loader
            libGL
          ]);
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          PHOSPHOR_ICONS = "${phosphorIcons}";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages =
            commonArgs.buildInputs
            ++ commonArgs.nativeBuildInputs
            ++ (with pkgs; [
              (rust-bin.stable.latest.default.override
                {
                  extensions = ["rustfmt" "rustc" "rust-analyzer" "cargo" "rust-src"];
                })
              cargo-hakari
              cargo-expand
              cargo-machete
              prek
              uv
              python314
              libGL
              (nvim.lib.makeNeovimWithLanguages {
                inherit pkgs system;
                languages.rust = {
                  enable = true;
                  toolchain = rustToolchain;
                };
              })
            ]);
        };
      }
    );
  in
    systems
    // {
      overlays.default = overlay;
      homeManagerModules.subniri = import ./nix/home-manager/subniri.nix {inherit self;};
      homeManagerModules.default = self.homeManagerModules.subniri;
      homeModules.subniri = self.homeManagerModules.subniri;
    };
}
