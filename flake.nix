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

  outputs = {
    self,
    nixpkgs,
    crane,
    flake-utils,
    rust-overlay,
    nvim,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };

        rustToolchain = p:
          p.rust-bin.stable.latest.default.override {
            extensions = ["rustfmt" "rustc" "rust-analyzer" "cargo"];
          };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        inherit (pkgs) lib;
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./crates/bar)
            (craneLib.fileset.commonCargoSources ./crates/cli)
            (craneLib.fileset.commonCargoSources ./crates/icons)
            (craneLib.fileset.commonCargoSources ./crates/launcher)
            (craneLib.fileset.commonCargoSources ./crates/niri-client)
            (craneLib.fileset.commonCargoSources ./crates/process-guard)
            (craneLib.fileset.commonCargoSources ./crates/workspace-hack)
            (craneLib.fileset.commonCargoSources ./crates/xtask)
            (lib.fileset.fileFilter (file: file.hasExt "blp") ./.)
            (lib.fileset.maybeMissing ./assets)
            (lib.fileset.maybeMissing ./systemd)
            ./style.css
            ./Cargo.toml
            ./Cargo.lock
          ];
        };

        # Common arguments can be set here to avoid repeating them later
        # Note: changes here will rebuild all dependency crates
        commonArgs = {
          # Keep Cargo sources while also including assets/*.gif and resources.xml
          inherit src;
          strictDeps = true;

          buildInputs = with pkgs; [
            astal.apps
            astal.io
            astal.tray
            astal.mpris
            astal.notifd
            astal.cava
            astal.wireplumber
            astal.network
            astal.bluetooth
            astal.astal4
            gtk4
            gtk4-layer-shell
            json-glib
            networkmanager
            graphene
            glib-networking
            gvfs
            libglycin
            libgweather
            glycin-loaders
            lcms
            bubblewrap
            cacert
            gnutls
            gsettings-desktop-schemas
            appmenu-glib-translator
          ];

          nativeBuildInputs = with pkgs; [
            pkg-config
            pre-commit
            ags
            blueprint-compiler
            glib
            wrapGAppsHook4
          ];
          # Disable all checks; otherwise pytestCheckHook from dependencies runs
          # and fails because there are no Python tests here.
          doCheck = false;
          doInstallCheck = false;
          checkPhase = "true";
          installCheckPhase = "true";
          nativeCheckInputs = [];
        };

        lucideIcons = pkgs.stdenv.mkDerivation {
          name = "lucide-icons-gtk";

          src = pkgs.fetchzip {
            url = "https://github.com/lucide-icons/lucide/releases/download/0.561.0/lucide-icons-0.561.0.zip";
            sha256 = "sha256-ReN9IKZMBuSlkKTsG6JEYPQi5ctirXv54t+Q5h5PaX4=";
          };

          installPhase = ''
            mkdir -p $out
            cp -r * $out/

            for svg in $(find $out -name "*.svg" ! -name "*-filled.svg" -type f); do
              filled="''${svg%.svg}-filled.svg"
              cp "$svg" "$filled"
            done

            find $out -name "*.svg" ! -name "*-filled.svg" -type f -exec sed -i 's/<path /<path class="foreground-stroke transparent-fill" /g' {} +

            find $out -name "*-filled.svg" -type f -exec sed -i \
              -e 's/class="foreground-stroke transparent-fill" //g' \
              -e 's/fill="none"/fill="currentColor"/g' \
              -e 's/stroke="currentColor"/stroke="none"/g' \
              {} +
          '';
        };
        simpleIcons = pkgs.fetchFromGitHub {
          owner = "simple-icons";
          repo = "simple-icons";
          rev = "16.2.0";
          hash = "sha256-bDOiWqonxrcuc5fLvm6p+Y0KpcKlrZibaLROkpfA+PU=";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs =
          commonArgs
          // {
            inherit cargoArtifacts;
            inherit (craneLib.crateNameFromCargoToml {inherit src;}) version;
            doCheck = false;
          };

        subniri = craneLib.buildPackage {
          inherit src;
          inherit (craneLib.crateNameFromCargoToml {inherit src;}) version;
          strictDeps = true;

          buildInputs = [
            pkgs.dbus
          ];
          pname = "subniri";
          cargoExtraArgs = "-p cli --bin subniri";
        };

        polarbar = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "polarbar";
            cargoExtraArgs = "-p bar --bin polarbar";

            LUCIDE_ICONS_PATH = "${lucideIcons}";
            SIMPLE_ICONS_PATH = "${simpleIcons}/icons";

            preFixup = ''
              gappsWrapperArgs+=(
                --set GLYCIN_LOADERS_PATH ${pkgs.glycin-loaders}/libexec/glycin-loaders/2+
                --prefix XDG_DATA_DIRS : ${pkgs.glycin-loaders}/share
                --prefix PATH : ${pkgs.bubblewrap}/bin
              )
            '';
          }
        );

        avalaunch = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "avalaunch";
            cargoExtraArgs = "-p launcher --bin avalaunch";
          }
        );
      in {
        checks = {
          inherit subniri polarbar avalaunch;

          subniri-workspace-hakari = craneLib.mkCargoDerivation {
            inherit src;
            pname = "subniri-workspace-hakari";
            version = "0.1.0";
            cargoArtifacts = null;
            doInstallCargoArtifacts = false;

            buildPhaseCargoCommand = ''
              cargo hakari generate --diff
              cargo hakari manage-deps --dry-run
              cargo hakari verify
            '';

            nativeBuildInputs = [
              pkgs.cargo-hakari
            ];
          };
        };

        packages = {
          inherit subniri polarbar avalaunch;
          default = subniri;
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = subniri;
            name = "subniri";
          };

          subniri = flake-utils.lib.mkApp {
            drv = subniri;
            name = "subniri";
          };

          polarbar = flake-utils.lib.mkApp {
            drv = polarbar;
            name = "polarbar";
          };

          avalaunch = flake-utils.lib.mkApp {
            drv = avalaunch;
            name = "avalaunch";
          };
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};

          shellHook = with pkgs; ''
            export XDG_DATA_DIRS="${astal.notifd}/share:${astal.notifd}/share/gsettings-schemas/${astal.notifd.name}:${gsettings-desktop-schemas}/share:${glycin-loaders}/share:${glib-networking}/share:${gvfs}/share:${libgweather}/share:${libgweather}/share/gsettings-schemas/${libgweather.name}:$XDG_DATA_DIRS"
            export GIO_EXTRA_MODULES="${glib-networking}/lib/gio/modules:${gvfs}/lib/gio/modules:$GIO_EXTRA_MODULES"
          '';

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

          LD_LIBRARY_PATH = with pkgs;
            lib.makeLibraryPath [
              astal.apps
              astal.io
              astal.tray
              astal.mpris
              astal.notifd
              astal.cava
              astal.wireplumber
              astal.network
              astal.bluetooth
              astal.astal4
              gtk4
              gtk4-layer-shell
              glib
              json-glib
              networkmanager
              graphene
              libglycin
              glycin-loaders
              lcms
              fontconfig
              libseccomp
              glib-networking
              gvfs
              gnutls
              gsettings-desktop-schemas
              libgweather
              appmenu-glib-translator
            ];

          LUCIDE_ICONS_PATH = "${lucideIcons}";
          SIMPLE_ICONS_PATH = "${simpleIcons}/icons";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [
            pkgs.cargo-hakari
            (nvim.lib.${system}.makeNeovimWithLanguages {
              inherit pkgs;
              languages.rust = {
                enable = true;
                toolchain = rustToolchain pkgs;
              };
            })
          ];
        };
      }
    );
}
