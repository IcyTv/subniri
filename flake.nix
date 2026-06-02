{
  description = "Build a cargo workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    cargo2nix.url = "github:cargo2nix/cargo2nix";

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

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = ["rustfmt" "rustc" "rust-analyzer" "cargo" "rust-src"];
        };

        rustPkgs = pkgs.rustBuilder.makePackageSet {
          packageFun = import ./Cargo.nix;
          workspaceSrc = ./.;

          inherit rustToolchain;
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
    };
}
