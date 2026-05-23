{
  description = "Build a cargo workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nvim.url = "github:IcyTv/nvim.nix";
    nvim.inputs.nixpkgs.follows = "nixpkgs";
  };

  nixConfig = {
    extra-substituters = ["https://icytv.cachix.org"];
    extra-trusted-public-keys = ["icytv.cachix.org-1:epXlDqA5apfoHPIc+Z7Vx6aPN7Tsz2hzik62V5Rs5sQ="];
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    flake-utils,
    rust-overlay,
    nvim,
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
          overlays = [(import rust-overlay)];
          config.allowUnfreePredicate = pkg:
            builtins.elem (nixpkgs.lib.getName pkg) [
              "cmp-spell"
            ];
        };

        rustToolchain = p:
          p.rust-bin.stable.latest.default.override {
            extensions = ["rustfmt" "rustc" "rust-analyzer" "cargo" "rust-src"];
          };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        inherit (pkgs) lib;
        rustSrc = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            # ./CMakeLists.txt
            # (craneLib.fileset.commonCargoSources ./crates/niri)
            # (craneLib.fileset.commonCargoSources ./crates/location_provider)
            # (craneLib.fileset.commonCargoSources ./crates/gammarelay)
            # (craneLib.fileset.commonCargoSources ./crates/login_manager)
            # (craneLib.fileset.commonCargoSources ./crates/spotify)
            # (craneLib.fileset.commonCargoSources ./crates/homeassistant)
            # (craneLib.fileset.commonCargoSources ./crates/kdl_adapter)
            # (craneLib.fileset.commonCargoSources ./crates/oauth)
            # (craneLib.fileset.commonCargoSources ./crates/secret_store)
            # (craneLib.fileset.commonCargoSources ./crates/workspace-hack)
            # ./Cargo.toml
            # ./Cargo.lock
          ];
        };

        # cargoVendorDir = craneLib.vendorCargoDeps {
        #   src = rustSrc;
        # };

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
          ];
          # Disable all checks; otherwise pytestCheckHook from dependencies runs
          # and fails because there are no Python tests here.
          doCheck = false;
          doInstallCheck = false;
          checkPhase = "true";
          installCheckPhase = "true";
          nativeCheckInputs = [];
        };

        phosphorIcons = pkgs.stdenv.mkDerivation {
          name = "phosphor-icons-qt";

          src = pkgs.fetchzip {
            url = "https://phosphoricons.com/assets/phosphor-icons.zip";
            sha256 = "sha256-vrYPR6pzPO6b8lQI8/kY1pT8txj6KDampIg3qcgiAL8=";
            stripRoot = false;
          };

          installPhase = ''
              mkdir -p $out
              cp -r SVGs\ Flat/* $out/

              cd $out

              find . -type f -name "*.svg" | sort | \
            awk '
            BEGIN {
              print "<RCC>"
              print "  <qresource prefix=\"icons\">"
            }
            {
              gsub(/^\.\//, "", $0)
              print "    <file>" $0 "</file>"
            }
            END {
              print "  </qresource>"
              print "</RCC>"
            }
            ' > icons.qrc
          '';
        };
        # subniri-cxxqt-modules = pkgs.stdenv.mkDerivation (commonArgs
        #   // {
        #     pname = "subniri-cxxqt-modules";
        #     inherit version;
        #     src = rustSrc;
        #
        #     nativeBuildInputs =
        #       commonArgs.nativeBuildInputs
        #       ++ [
        #         (rustToolchain pkgs)
        #       ];
        #
        #     postPatch = ''
        #       mkdir -p .cargo
        #       cp ${cargoVendorDir}/config.toml .cargo/config.toml
        #     '';
        #
        #     CARGO_NET_OFFLINE = "true";
        #   });
      in {
        checks = {
          # inherit subniri-cxxqt-modules;

          #   subniri-workspace-hakari = craneLib.mkCargoDerivation {
          #     src = rustSrc;
          #     pname = "subniri-workspace-hakari";
          #     version = "0.1.0";
          #     cargoArtifacts = null;
          #     doInstallCargoArtifacts = false;
          #
          #     buildPhaseCargoCommand = ''
          #       cargo hakari generate --diff
          #       cargo hakari manage-deps --dry-run
          #     '';
          #
          #     nativeBuildInputs = [
          #       pkgs.cargo-hakari
          #     ];
          #   };
        };

        packages = {
          # inherit subniri-cxxqt-modules;
          # subniri-cargo-vendor = cargoVendorDir;

          # default = subniri-shell;
        };

        apps = {
          default = flake-utils.lib.mkApp {
            # drv = subniri-shell;
            name = "subniri-shell";
          };

          # subniri-shell = flake-utils.lib.mkApp {
          #   drv = subniri-shell;
          #   name = "subniri-shell";
          # };
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};
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
              pkgs.cargo-hakari
              pkgs.cargo-expand
              (nvim.lib.makeNeovimWithLanguages {
                inherit pkgs system;
                languages.rust = {
                  enable = true;
                  toolchain = rustToolchain pkgs;
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
      # homeModules.subniri = import ./nix/home-manager/subniri.nix {inherit self;};
      # homeManagerModules.subniri = import ./nix/home-manager/subniri.nix {inherit self;};
      # homeManagerModules.default = self.homeManagerModules.subniri;
    };
}
