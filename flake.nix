{
  description = "Experimental native Eye Tracker 5 driver for Linux";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }: let
    systems = [ "x86_64-linux" "aarch64-linux" ];
    forAllSystems = nixpkgs.lib.genAttrs systems;
  in {
    packages = forAllSystems (system: let
      pkgs = import nixpkgs { inherit system; };
      lib = pkgs.lib;
      repoModels = ./models/tobii-et5;
      bridge = import ./nix/bridge.nix { inherit pkgs; };
    in rec {
      prp-bridge = bridge.prpBridge;
      wine-run = bridge.wineRun;
      tobii-setup = pkgs.rustPlatform.buildRustPackage {
        pname = "tobii-setup";
        version = "0.1.0";
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./clippy.toml ./rustfmt.toml ./crates ];
        };
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "tobii-setup" "--features" "gtk" ];
        cargoTestFlags = [ "-p" "tobii-setup" "--features" "gtk" ];
        nativeBuildInputs = [ pkgs.pkg-config pkgs.wrapGAppsHook4 ];
        buildInputs = [ pkgs.gtk4 ];
        meta = {
          description = "Guided display setup and calibration of the Eye Tracker 5 (GTK 4)";
          platforms = lib.platforms.linux;
          mainProgram = "tobii-linux-setup";
        };
      };
      default = pkgs.python3Packages.buildPythonApplication {
        pname = "tobii-linux";
        version = "0.1.0";
        pyproject = true;
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            ./pyproject.toml ./Cargo.toml ./Cargo.lock ./clippy.toml ./rustfmt.toml ./crates ./src ./tests ./tools
          ];
        };
        cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
        nativeBuildInputs = with pkgs.rustPlatform; [ cargoSetupHook maturinBuildHook ];
        preConfigure = ''
          export HOME="$NIX_BUILD_TOP/home"
          mkdir -p "$HOME"
        '';
        env.TOBII_OPENVINO_LIBRARY_DIR = "${lib.getLib pkgs.openvino}/lib";
        dependencies = with pkgs.python3Packages; [ numpy openvino ];
        makeWrapperArgs = [ "--set-default TOBII_LINUX_MODELS ${models}" ];
        checkPhase = ''
          runHook preCheck
          mkdir -p models && ln -s ${models} models/tobii-et5
          cargo test --release --workspace --offline --frozen --target ${pkgs.stdenv.hostPlatform.rust.rustcTarget}
          PYTHONPATH=$out/${pkgs.python3.sitePackages}:$PYTHONPATH python -m unittest discover -s tests
          runHook postCheck
        '';
        pythonImportsCheck = [ "tobii_linux.usb" "tobii_linux._native" ];
        meta = {
          description = "Experimental Eye Tracker 5 head-pose driver";
          platforms = pkgs.lib.platforms.linux;
          mainProgram = "tobii-linux";
        };
      };
    } // lib.optionalAttrs (builtins.pathExists repoModels) {
      modelsFromRepo = pkgs.runCommand "tobii-linux-models-from-repo" { } ''
        mkdir -p $out
        cp -r ${repoModels}/. $out/
      '';
    });
    nixosModules.default = import ./nix/module.nix self;
    overlays.wine = import ./nix/wine.nix;
    checks = forAllSystems (system: let
      pkgs = import nixpkgs { inherit system; };
      machine = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          self.nixosModules.default
          {
            hardware.tobii-eye-tracker-5 = {
              enable = true;
              headPoseService = {
                enable = true;
                displayArea = [ (-296.62) 326 111.82 300.38 326 111.82 (-296.62) 10.27 (-3.1) ];
              };
            };
            boot.loader.grub.enable = false;
            fileSystems."/" = { device = "/dev/null"; fsType = "ext4"; };
            system.stateVersion = "25.11";
          }
        ];
      };
      names = map (p: p.name or "") ;
      installed = builtins.elem self.packages.${system}.default.name (names machine.config.environment.systemPackages);
      ruled = builtins.elem "tobii-eye-tracker-5-udev" (names machine.config.services.udev.packages);
      served = machine.config.systemd.user.services.tobii-head-pose.serviceConfig.ExecStart != "";
    in {
      package = self.packages.${system}.default;
      nixos-module = assert installed && ruled && served; pkgs.runCommand "tobii-linux-nixos-module-eval" { } "touch $out";
    } // nixpkgs.lib.optionalAttrs (system == "x86_64-linux") {
      wine-run = self.packages.${system}.wine-run;
    });
  };
}
