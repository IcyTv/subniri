{
  description = "Build a cargo project without extra checks";

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

        rustToolchain = p: p.rust-bin.stable.latest.default;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Common arguments can be set here to avoid repeating them later
        # Note: changes here will rebuild all dependency crates
        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;

          buildInputs = with pkgs;
            [
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
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              # Additional darwin specific inputs can be set here
              pkgs.libiconv
            ];

           nativeBuildInputs = with pkgs; [
            pkg-config
            pre-commit
            ags
            blueprint-compiler
            glib
            wrapGAppsHook4
           ];

           doCheck = false;
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

        niribar = craneLib.buildPackage (
          commonArgs
          // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;

            # Additional environment variables or build phases/hooks can be set
            # here *without* rebuilding all dependency crates
            # MY_CUSTOM_VAR = "some value";

            LUCIDE_ICONS_PATH = "${lucideIcons}";
            SIMPLE_ICONS_PATH = "${simpleIcons}/icons";
          }
        );
      in {
        checks = {
          inherit niribar;
        };

        packages.default = niribar;

        apps.default = flake-utils.lib.mkApp {
          drv = niribar;
          name = "niribar";
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
            ];

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [
            # pkgs.ripgrep
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
