# Overlay that adds Wine builds carrying the named-pipe listing patch, for use as
# a Lutris or launch-script runner (Star Citizen's Tobii layer only tracks
# trackers it finds by listing \\.\pipe\*):
#
#   nixpkgs.overlays = [ (import ./nix/wine.nix) ];
#   environment.systemPackages = [ pkgs.tobii-linux-wine.staging ];
#
# patchedSourceCheck applies the patch to each source without building Wine.
final: prev:
let
  inherit (prev) lib;
  winePackages = prev.wineWow64Packages or prev.wineWowPackages;
  applyScript = ../patches/wine;

  withPipeListing = wine: wine.overrideAttrs (old: {
    pname = old.pname + "-tobii-pipe-listing";
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ final.perl ];
    postPatch = (old.postPatch or "") + ''
      patchShebangs tools/make_requests
      bash ${applyScript}/apply.sh .
    '';
  });

  patchedSourceCheck = name: wine: (withPipeListing wine).overrideAttrs {
    pname = "tobii-pipe-listing-applies-to-${name}";
    phases = [ "unpackPhase" "patchPhase" "installPhase" ];
    installPhase = ''
      grep -q get_named_pipe_entries include/wine/server_protocol.h
      mkdir -p $out
      cp server/protocol.def include/wine/server_protocol.h $out/
    '';
  };
in
{
  tobii-linux-wine = {
    stable = withPipeListing winePackages.stable;
    staging = withPipeListing winePackages.staging;
    unstable = withPipeListing winePackages.unstable;
    patchedSourceCheck = lib.mapAttrs patchedSourceCheck {
      inherit (winePackages) stable staging unstable;
    };
  };
}
