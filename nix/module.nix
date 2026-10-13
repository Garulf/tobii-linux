self: { config, lib, pkgs, ... }:
let cfg = config.hardware.tobii-eye-tracker-5;
in {
  options.hardware.tobii-eye-tracker-5.enable = lib.mkEnableOption
    "Eye Tracker 5 USB access and the experimental head-pose command";
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
    # Runs before 73-seat-late.rules; logind grants the active local seat user
    # access. This deliberately does not grant all users write access.
    services.udev.packages = [ (pkgs.writeTextFile {
      name = "tobii-eye-tracker-5-udev";
      destination = "/lib/udev/rules.d/70-tobii-eye-tracker-5.rules";
      text = ''
        SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="2104", ATTR{idProduct}=="0313", TAG+="uaccess"
      '';
    }) ];
  };
}
