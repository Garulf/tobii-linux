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
        nativeCheckInputs = [ pkgs.python3Packages.pytest ];
        checkPhase = ''
          PYTHONPATH=src python -m pytest tests
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
    checks = forAllSystems (system: { package = self.packages.${system}.default; });
  };
}
