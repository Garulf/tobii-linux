{
  description = "Experimental native Eye Tracker 5 driver for Linux";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }: let
    systems = [ "x86_64-linux" "aarch64-linux" ];
    forAllSystems = nixpkgs.lib.genAttrs systems;
  in {
    packages = forAllSystems (system: let pkgs = import nixpkgs { inherit system; }; in rec {
      models = pkgs.runCommand "tobii-linux-models" { } ''
        mkdir -p $out
        cp -r ${./models/tobii-et5}/. $out/
      '';
      default = pkgs.python3Packages.buildPythonApplication {
        pname = "tobii-linux";
        version = "0.1.0";
        pyproject = true;
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [ ./pyproject.toml ./src ./tests ];
        };
        build-system = [ pkgs.python3Packages.setuptools ];
        dependencies = [ pkgs.python3Packages.numpy pkgs.python3Packages.openvino ];
        makeWrapperArgs = [
          "--set-default TOBII_LIBUSB_PATH ${pkgs.libusb1}/lib/libusb-1.0.so"
          "--set-default TOBII_LINUX_MODELS ${models}"
        ];
        checkPhase = ''
          PYTHONPATH=src python -m unittest discover -s tests
        '';
        pythonImportsCheck = [ "tobii_linux.usb" ];
        meta = {
          description = "Experimental Eye Tracker 5 head-pose driver";
          platforms = pkgs.lib.platforms.linux;
          mainProgram = "tobii-linux";
        };
      };
    });
    nixosModules.default = import ./nix/module.nix self;
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
    });
  };
}
