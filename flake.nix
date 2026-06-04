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
    version = "0.1.0";

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

        rustToolchain = pkgs.rust-bin.stable.latest.minimal;

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
              (pkgs.rustBuilder.rustLib.makeOverride {
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
              (pkgs.rustBuilder.rustLib.makeOverride {
                name = "smithay-client-toolkit";
                overrideAttrs = drv: {
                  nativeBuildInputs =
                    (drv.nativeBuildInputs or [])
                    ++ [
                      pkgs.pkg-config
                    ];
                  buildInputs =
                    (drv.buildInputs or [])
                    ++ [
                      pkgs.libxkbcommon
                    ];
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                name = "libpulse-sys";

                overrideAttrs = drv: {
                  propagatedBuildInputs =
                    (drv.propagatedBuildInputs or [])
                    ++ [
                      pkgs.libpulseaudio
                    ];
                  nativeBuildInputs =
                    (drv.nativeBuildInputs or [])
                    ++ [
                      pkgs.pkg-config
                    ];
                  postInstall =
                    (drv.postInstall or "")
                    + ''
                      rm -f $out/lib/.link-flags
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                registry = "git+https://github.com/iced-rs/winit.git";
                name = "dpi";

                overrideAttrs = drv: {
                  postPatch =
                    (drv.postPatch or "")
                    + ''
                      substituteInPlace Cargo.toml \
                        --replace 'rust-version.workspace = true' 'rust-version = "1.70.0"' \
                        --replace 'repository.workspace = true' 'repository = "https://github.com/rust-windowing/winit"' \
                        --replace 'license.workspace = true' 'license = "Apache-2.0"' \
                        --replace 'edition.workspace = true' 'edition = "2021"' \
                        --replace 'serde = { workspace = true, optional = true }' 'serde = { version = "1", features = ["serde_derive"], optional = true }' \
                        --replace 'mint = { workspace = true, optional = true }' 'mint = { version = "0.5.6", optional = true }'
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                registry = "git+https://github.com/iced-rs/winit.git";
                name = "winit";

                overrideAttrs = drv: {
                  postPatch =
                    (drv.postPatch or "")
                    + ''
                      substituteInPlace Cargo.toml \
                        --replace 'rust-version.workspace = true' 'rust-version = "1.70.0"' \
                        --replace 'repository.workspace = true' 'repository = "https://github.com/rust-windowing/winit"' \
                        --replace 'license.workspace = true' 'license = "Apache-2.0"' \
                        --replace 'edition.workspace = true' 'edition = "2021"' \
                        --replace 'serde = { workspace = true, optional = true }' 'serde = { version = "1", features = ["serde_derive"], optional = true }'

                      substituteInPlace dpi/Cargo.toml \
                        --replace 'rust-version.workspace = true' 'rust-version = "1.70.0"' \
                        --replace 'repository.workspace = true' 'repository = "https://github.com/rust-windowing/winit"' \
                        --replace 'license.workspace = true' 'license = "Apache-2.0"' \
                        --replace 'edition.workspace = true' 'edition = "2021"' \
                        --replace 'serde = { workspace = true, optional = true }' 'serde = { version = "1", features = ["serde_derive"], optional = true }' \
                        --replace 'mint = { workspace = true, optional = true }' 'mint = { version = "0.5.6", optional = true }'
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                registry = "git+https://github.com/IcyTv/iced";
                name = "iced";

                overrideAttrs = drv: {
                  postPatch =
                    (drv.postPatch or "")
                    + ''
                      substituteInPlace Cargo.toml \
                        --replace 'rust-version.workspace = true' 'rust-version = "1.92"' \
                        --replace 'version.workspace = true' 'version = "0.15.0-dev"' \
                        --replace 'edition.workspace = true' 'edition = "2024"' \
                        --replace 'authors.workspace = true' 'authors = []' \
                        --replace 'license.workspace = true' 'license = "MIT"' \
                        --replace 'repository.workspace = true' 'repository = "https://github.com/iced-rs/iced"' \
                        --replace 'homepage.workspace = true' 'homepage = "https://iced.rs"' \
                        --replace 'categories.workspace = true' 'categories = ["gui"]' \
                        --replace 'keywords.workspace = true' 'keywords = ["gui", "ui", "graphics", "interface", "widgets"]' \
                        --replace 'iced_debug.workspace = true' 'iced_debug = { version = "0.15.0-dev", path = "debug" }' \
                        --replace 'iced_core.workspace = true' 'iced_core = { version = "0.15.0-dev", path = "core" }' \
                        --replace 'iced_futures.workspace = true' 'iced_futures = { version = "0.15.0-dev", path = "futures" }' \
                        --replace 'iced_renderer.workspace = true' 'iced_renderer = { version = "0.15.0-dev", path = "renderer" }' \
                        --replace 'iced_runtime.workspace = true' 'iced_runtime = { version = "0.15.0-dev", path = "runtime" }' \
                        --replace 'iced_widget.workspace = true' 'iced_widget = { version = "0.15.0-dev", path = "widget" }' \
                        --replace 'iced_winit.workspace = true' 'iced_winit = { version = "0.15.0-dev", path = "winit" }' \
                        --replace 'thiserror.workspace = true' 'thiserror = "2"'

                      substituteInPlace Cargo.toml \
                        --replace 'iced_devtools.workspace = true' 'iced_devtools = { version = "0.15.0-dev", path = "devtools", optional = true }' \
                        --replace 'iced_devtools.optional = true' '# optional moved into inline table' \
                        --replace 'iced_tester.workspace = true' 'iced_tester = { version = "0.15.0-dev", path = "tester", optional = true }' \
                        --replace 'iced_tester.optional = true' '# optional moved into inline table' \
                        --replace 'iced_highlighter.workspace = true' 'iced_highlighter = { version = "0.15.0-dev", path = "highlighter", optional = true }' \
                        --replace 'iced_highlighter.optional = true' '# optional moved into inline table' \
                        --replace 'image.workspace = true' 'image = { version = "0.25", default-features = false, optional = true }' \
                        --replace 'image.optional = true' '# optional moved into inline table' \
                        --replace 'iced_wgpu.workspace = true' 'iced_wgpu = { version = "0.15.0-dev", path = "wgpu" }' \
                        --replace 'workspace = true' '# removed workspace lint inheritance for cargo2nix'
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                registry = "unknown";

                overrideAttrs = _drv: {
                  PHOSPHOR_ICONS = "${phosphorIcons}";
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                name = "polarbar";

                overrideAttrs = drv: {
                  nativeBuildInputs =
                    (drv.nativeBuildInputs or [])
                    ++ [
                      pkgs.makeWrapper
                    ];

                  buildInputs =
                    (drv.buildInputs or [])
                    ++ [
                      pkgs.wayland
                      pkgs.libxkbcommon
                      pkgs.pipewire
                    ];

                  SUBNIRI_ICEOUT_BIN = "${rustPkgs.workspace.iceout {}}/bin/iceout";
                  SUBNIRI_SNOWCONF_BIN = "${rustPkgs.workspace.settings {}}/bin/snowconf";

                  postInstall =
                    (drv.postInstall or "")
                    + ''
                      wrapProgram $bin/bin/polarbar \
                        --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [pkgs.wayland pkgs.libxkbcommon pkgs.pipewire pkgs.vulkan-loader]} \
                        --set XKB_CONFIG_ROOT "${pkgs.xkeyboard_config}/share/X11/xkb" \
                        --set SPA_PLUGIN_DIR "${pkgs.pipewire}/lib/spa-0.2"
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                name = "iceout";

                overrideAttrs = drv: {
                  nativeBuildInputs =
                    (drv.nativeBuildInputs or [])
                    ++ [
                      pkgs.makeWrapper
                    ];

                  buildInputs =
                    (drv.buildInputs or [])
                    ++ [
                      pkgs.wayland
                      pkgs.libxkbcommon
                    ];

                  postInstall =
                    (drv.postInstall or "")
                    + ''
                      wrapProgram $bin/bin/iceout \
                        --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [pkgs.wayland pkgs.libxkbcommon pkgs.vulkan-loader]} \
                        --set XKB_CONFIG_ROOT "${pkgs.xkeyboard_config}/share/X11/xkb"
                    '';
                };
              })
              (pkgs.rustBuilder.rustLib.makeOverride {
                name = "settings";

                overrideAttrs = drv: {
                  nativeBuildInputs =
                    (drv.nativeBuildInputs or [])
                    ++ [
                      pkgs.makeWrapper
                    ];

                  buildInputs =
                    (drv.buildInputs or [])
                    ++ [
                      pkgs.wayland
                      pkgs.libxkbcommon
                    ];

                  postInstall =
                    (drv.postInstall or "")
                    + ''
                      wrapProgram $bin/bin/snowconf \
                        --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [pkgs.wayland pkgs.libxkbcommon pkgs.vulkan-loader]} \
                        --set XKB_CONFIG_ROOT "${pkgs.xkeyboard_config}/share/X11/xkb"
                    '';
                };
              })
            ];
        };

        inherit (pkgs) lib;

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
      in {
        packages = {
          subniri-cli = rustPkgs.workspace.cli {};
          polarbar = rustPkgs.workspace.polarbar {};
          permafrostd = rustPkgs.workspace.daemon {};
          avalaunch = rustPkgs.workspace.launcher {};
          iceout = rustPkgs.workspace.iceout {};
          snowconf = rustPkgs.workspace.settings {};

          default = pkgs.symlinkJoin {
            name = "subniri";

            paths = [
              (rustPkgs.workspace.cli {})
              (rustPkgs.workspace.polarbar {})
              (rustPkgs.workspace.daemon {})
              (rustPkgs.workspace.iceout {})
              (rustPkgs.workspace.launcher {})
              (rustPkgs.workspace.settings {})
            ];
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
          LATO_FONTS = "${pkgs.lato}/share/fonts/lato";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages =
            commonArgs.buildInputs
            ++ commonArgs.nativeBuildInputs
            ++ [
              (pkgs.rust-bin.stable.latest.default.override
                {
                  extensions = ["rustfmt" "rustc" "rust-analyzer" "cargo" "rust-src"];
                })
              pkgs.cargo-hakari
              pkgs.cargo-expand
              pkgs.libGL
              (nvim.lib.makeNeovimWithLanguages {
                inherit pkgs system;
                languages.rust = {
                  enable = true;
                  toolchain = rustToolchain;
                };
                languages.slint.enable = true;
                # languages.qml.enable = true;
              })
            ];
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
