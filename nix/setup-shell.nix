# The shell `make setup-app` and `make setup-test` build tobii-setup in when the host has
# no GTK 4: nixpkgs' Rust toolchain (proc macros must load against the same glibc as the
# GTK libraries), GTK 4, DejaVu Sans as the only font (the faces the browser page is drawn
# with on hosts without Segoe UI), and xdotool for headless runs on Xvfb.
#
#   nix-shell nix/setup-shell.nix --run 'cargo build -p tobii-setup --features gtk'
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  packages = with pkgs; [ cargo rustc clippy rustfmt pkg-config gtk4 xdotool ];
  FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts.minimal ]; };
  GSETTINGS_SCHEMA_DIR = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}/glib-2.0/schemas";
}
