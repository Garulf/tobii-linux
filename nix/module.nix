self: { config, lib, pkgs, ... }:
let
  cfg = config.hardware.tobii-eye-tracker-5;
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  wineRun = (import ./bridge.nix { inherit pkgs; }).wineRun;
  serveArguments = [
    "serve"
  ] ++ lib.optionals (cfg.headPoseService.displayArea != null) ([
    "--display"
  ] ++ map toString cfg.headPoseService.displayArea) ++ [
    "--wine-clock" cfg.headPoseService.wineClock
    "--port" (toString cfg.headPoseService.port)
    "--name" cfg.headPoseService.deviceName
  ] ++ lib.optionals (cfg.headPoseService.displayId != null) [
    "--display-id" cfg.headPoseService.displayId
  ] ++ lib.optionals cfg.headPoseService.tetTcp [ "--tet-tcp" ];
in {
  options.hardware.tobii-eye-tracker-5 = {
    enable = lib.mkEnableOption
      "Eye Tracker 5 USB access and the experimental head-pose command";
    headPoseService = {
      enable = lib.mkEnableOption
        "a systemd user service that serves head pose to Windows games through the Wine pipe bridge";
      displayArea = lib.mkOption {
        type = lib.types.nullOr
          (lib.types.addCheck (lib.types.listOf lib.types.number) (corners: builtins.length corners == 9));
        default = null;
        example = [ (-296.62) 326 111.82 300.38 326 111.82 (-296.62) 10.27 (-3.1) ];
        description = ''
          Display corners in millimetres relative to the tracker, in Stream
          Engine order: top-left, top-right, bottom-left, three coordinates each.
          null uses the display area stored on the tracker (set it with
          tobii-linux setup); a list overrides it.
        '';
      };
      wineClock = lib.mkOption {
        type = lib.types.enum [ "auto" "boottime" "raw" ];
        default = "auto";
        description = ''
          Clock behind the game's Wine QueryPerformanceCounter. auto follows
          the counter the bridge in the game's prefix reports; boottime fixes
          it for Wine 10.17 and later, raw for older releases and for every
          Proton and GE-Proton.
        '';
      };
      port = lib.mkOption {
        type = lib.types.port;
        default = 7760;
        description = "Loopback TCP port the bridge in the Wine prefix connects to.";
      };
      displayId = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Monitor device instance id as seen inside the game's Wine prefix.
          Only needed when the game tracks a window rather than the tracker
          itself; see docs/game-integration-layer.md.
        '';
      };
      models = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/home/alice/tobii-et5";
        description = ''
          Directory holding the models/tobii-et5 layout (alignpre, fr, ff, d1,
          support/ and catalog.json), passed to the service as
          TOBII_LINUX_MODELS. The models are not part of this project. With
          null the binary's default, ~/.local/share/tobii-linux/models of the
          user running the service, is used.
        '';
      };
      deviceName = lib.mkOption {
        type = lib.types.str;
        default = "ET5-LINUX";
        description = "Device name the game sees as tobii-prp://<name>.";
      };
      tetTcp = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Serve the route that works with an unpatched Wine or Proton instead
          of the PRP pipes: gaze on TCP 127.0.0.1:4455 and head pose over the
          bridge's streamengineservices pipe. Experimental; see docs/tet-tcp.md.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # tobii-linux-wine-run carries prp_bridge.exe, which only x86-64 Wine runs.
    environment.systemPackages = [ package ]
      ++ lib.optional pkgs.stdenv.hostPlatform.isx86_64 wineRun;
    # Runs before 73-seat-late.rules; logind grants the active local seat user
    # access. This deliberately does not grant all users write access.
    services.udev.packages = [ (pkgs.writeTextFile {
      name = "tobii-eye-tracker-5-udev";
      destination = "/lib/udev/rules.d/70-tobii-eye-tracker-5.rules";
      text = ''
        SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="2104", ATTR{idProduct}=="0313", TAG+="uaccess"
      '';
    }) ];
    systemd.user.services.tobii-head-pose = lib.mkIf cfg.headPoseService.enable {
      description = "Eye Tracker 5 head pose for Windows games under Wine";
      wantedBy = [ "default.target" ];
      environment = lib.mkIf (cfg.headPoseService.models != null) {
        TOBII_LINUX_MODELS = cfg.headPoseService.models;
      };
      serviceConfig = {
        ExecStart = lib.escapeShellArgs ([ (lib.getExe package) ] ++ serveArguments);
        Restart = "on-failure";
        RestartSec = 2;
      };
    };
  };
}
