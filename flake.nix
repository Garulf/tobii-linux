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
      bridge = import ./nix/bridge.nix { inherit pkgs; };
      rustSource = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./clippy.toml ./rustfmt.toml ./crates ];
      };
    in rec {
      prp-bridge = bridge.prpBridge;
      wine-run = bridge.wineRun;
      tobii-setup = pkgs.rustPlatform.buildRustPackage {
        pname = "tobii-setup";
        version = "0.1.0";
        src = rustSource;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "tobii-setup" "--features" "gtk" ];
        cargoTestFlags = [ "-p" "tobii-setup" "--features" "gtk" ];
        preConfigure = ''
          export HOME="$NIX_BUILD_TOP/home"
          mkdir -p "$HOME"
        '';
        nativeBuildInputs = [ pkgs.pkg-config pkgs.wrapGAppsHook4 ];
        buildInputs = [ pkgs.gtk4 ];
        meta = {
          description = "Guided display setup and calibration of the Eye Tracker 5 (GTK 4)";
          platforms = lib.platforms.linux;
          mainProgram = "tobii-linux-setup";
        };
      };
      selawik = import ./nix/selawik.nix { inherit pkgs; };
      tobii-experience = pkgs.rustPlatform.buildRustPackage {
        pname = "tobii-experience";
        version = "0.1.0";
        src = rustSource;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "tobii-experience" "--features" "gtk" ];
        cargoTestFlags = [ "-p" "tobii-experience" "--features" "gtk" ];
        preConfigure = ''
          export HOME="$NIX_BUILD_TOP/home"
          mkdir -p "$HOME"
        '';
        nativeBuildInputs = [ pkgs.pkg-config pkgs.wrapGAppsHook4 pkgs.copyDesktopItems ];
        buildInputs = [ pkgs.gtk4 pkgs.gtk4-layer-shell ];
        desktopItems = [ (pkgs.makeDesktopItem {
          name = "org.tobii_linux.Experience";
          desktopName = "Tobii Experience";
          comment = "Eye Tracker 5 status, on/off and the Eye tracking page";
          exec = "tobii-linux-experience --page";
          icon = "tobii-linux-experience";
          categories = [ "Settings" "HardwareSettings" ];
        }) ];
        postInstall = ''
          install -Dm644 crates/tobii-experience/assets/tray/Enabled.png $out/share/pixmaps/tobii-linux-experience.png
        '';
        # Selawik for the CSS, and the setup app the page and --guest start.
        preFixup = ''
          gappsWrapperArgs+=(
            --set-default FONTCONFIG_FILE ${pkgs.makeFontsConf { fontDirectories = [ selawik ]; }}
            --prefix PATH : ${lib.makeBinPath [ tobii-setup ]}
          )
        '';
        meta = {
          description = "Tobii Experience for Linux: tray icon, on/off, Eye tracking page, gaze preview (GTK 4)";
          platforms = lib.platforms.linux;
          mainProgram = "tobii-linux-experience";
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
        nativeBuildInputs = with pkgs.rustPlatform; [ cargoSetupHook maturinBuildHook pkgs.makeWrapper ];
        preConfigure = ''
          export HOME="$NIX_BUILD_TOP/home"
          mkdir -p "$HOME"
        '';
        env.TOBII_OPENVINO_LIBRARY_DIR = "${lib.getLib pkgs.openvino}/lib";
        dependencies = with pkgs.python3Packages; [ numpy openvino ];
        postBuild = ''
          cargo build --release --offline --frozen -p tobii-linux --bin tobii-linux --target ${pkgs.stdenv.hostPlatform.rust.rustcTarget}
        '';
        postInstall = ''
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/tobii-linux $out/bin/tobii-linux
        '';
        postFixup = ''
          ln -s ${lib.getExe tobii-setup} $out/bin/tobii-linux-setup
          ln -s ${lib.getExe tobii-experience} $out/bin/tobii-linux-experience
          mkdir -p $out/share/applications $out/share/pixmaps
          ln -s ${tobii-experience}/share/applications/org.tobii_linux.Experience.desktop $out/share/applications/
          ln -s ${tobii-experience}/share/pixmaps/tobii-linux-experience.png $out/share/pixmaps/
        '';
        checkPhase = ''
          runHook preCheck
          patchShebangs tools
          cargo test --release --workspace --offline --frozen --target ${pkgs.stdenv.hostPlatform.rust.rustcTarget} \
            -- --skip the_wrapper_starts_serve_for_the_game_and_stops_it_afterwards
          PYTHONPATH=$out/${pkgs.python3.sitePackages}:$PYTHONPATH python -m unittest discover -s tests
          $out/bin/tobii-linux serve --help | grep -q -- --tet-tcp
          runHook postCheck
        '';
        pythonImportsCheck = [ "tobii_linux.usb" "tobii_linux._native" "tobii_linux.cli" ];
        meta = {
          description = "Experimental Eye Tracker 5 head-pose driver";
          platforms = pkgs.lib.platforms.linux;
          mainProgram = "tobii-linux";
        };
      };
    });
    nixosModules.default = import ./nix/module.nix self;
    overlays.wine = import ./nix/wine.nix;
    checks = forAllSystems (system: let
      pkgs = import nixpkgs { inherit system; };
      machineWith = settings: nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          self.nixosModules.default
          {
            hardware.tobii-eye-tracker-5 = { enable = true; } // settings;
            boot.loader.grub.enable = false;
            fileSystems."/" = { device = "/dev/null"; fsType = "ext4"; };
            system.stateVersion = "25.11";
          }
        ];
      };
      headPoseWith = headPoseService: machineWith { headPoseService = { enable = true; } // headPoseService; };
      machine = headPoseWith {
        models = "/srv/tobii-et5";
        displayArea = [ (-296.62) 326 111.82 300.38 326 111.82 (-296.62) 10.27 (-3.1) ];
      };
      experienceMachine = machineWith { experience.enable = true; };
      userUnits = machine: machine.config.systemd.user.services;
      execStart = machine: (userUnits machine).tobii-head-pose.serviceConfig.ExecStart;
      names = map (p: p.name or "") ;
      installed = builtins.elem self.packages.${system}.default.name (names machine.config.environment.systemPackages);
      ruled = builtins.elem "tobii-eye-tracker-5-udev" (names machine.config.services.udev.packages);
      service = machine.config.systemd.user.services.tobii-head-pose;
      served = nixpkgs.lib.hasInfix "serve --display -296.62" (execStart machine);
      deviceArea = !(nixpkgs.lib.hasInfix "--display" (execStart (headPoseWith { })));
      modelled = service.environment.TOBII_LINUX_MODELS == "/srv/tobii-et5";
      noApp = !((userUnits machine) ? tobii-linux-experience);
      app = (userUnits experienceMachine).tobii-linux-experience;
      experienceServed = nixpkgs.lib.hasInfix "/bin/tobii-linux serve --wine-clock auto --port 7760 --name ET5-LINUX"
        (execStart experienceMachine);
      experienceStarted = builtins.elem "graphical-session.target" app.wantedBy
        && builtins.elem "graphical-session.target" app.partOf
        && builtins.elem "tobii-head-pose.service" app.after
        && nixpkgs.lib.hasSuffix "/bin/tobii-linux-experience --service 127.0.0.1:7760 --service-unit tobii-head-pose.service"
          app.serviceConfig.ExecStart;
    in {
      package = self.packages.${system}.default;
      experience = self.packages.${system}.tobii-experience;
      nixos-module = assert installed && ruled && served && deviceArea && modelled && noApp && experienceServed && experienceStarted;
        pkgs.runCommand "tobii-linux-nixos-module-eval" { } "touch $out";
    } // nixpkgs.lib.optionalAttrs (system == "x86_64-linux") {
      wine-run = self.packages.${system}.wine-run;
    });
  };
}
