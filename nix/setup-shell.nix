# The shell `make setup-app`, `make experience-app` and their test targets build the GTK 4
# apps in when the host has no GTK 4: nixpkgs' Rust toolchain (proc macros must load
# against the same glibc as the GTK libraries), GTK 4 and gtk4-layer-shell, DejaVu Sans as
# the only font (the faces the browser page is drawn with on hosts without Segoe UI),
# xdotool for headless runs on Xvfb, and dbus for the session bus the experience app's
# single-instance test runs on (dbus-run-session).
#
#   nix-shell nix/setup-shell.nix --run 'cargo build -p tobii-setup --features gtk'
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  packages = with pkgs; [ cargo rustc clippy rustfmt pkg-config gtk4 gtk4-layer-shell xdotool dbus ];
  FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts.minimal ]; };
  # dbus-run-session's default config is /etc/dbus-1/session.conf, which hosts without a
  # system D-Bus lack.
  DBUS_SESSION_CONFIG = "${pkgs.dbus}/share/dbus-1/session.conf";
  GSETTINGS_SCHEMA_DIR = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}/glib-2.0/schemas";
}
