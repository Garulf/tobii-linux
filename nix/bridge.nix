# prp_bridge.exe, cross-compiled with MinGW-w64, and tobii-linux-wine-run, the
# launcher that starts it in a game's Wine or Proton prefix.
#
#   bridge = import ./nix/bridge.nix { inherit pkgs; };
#   bridge.prpBridge        # $out/share/tobii-linux/prp_bridge.exe
#   bridge.wineRun          # $out/bin/tobii-linux-wine-run
#   (import ./nix/bridge.nix { inherit pkgs; withTestClients = true; }).prpBridge
{ pkgs, withTestClients ? false }:
let
  inherit (pkgs) lib;
  mingw = pkgs.pkgsCross.mingwW64;
  source = ../tools/prp_bridge;
  executables = [ "prp_bridge.exe" ]
    ++ lib.optionals withTestClients [ "prp_test_client.exe" "tgi_test_client.exe" ];

  prpBridge = mingw.stdenv.mkDerivation {
    pname = "tobii-linux-prp-bridge";
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = source;
      fileset = lib.fileset.unions ([ (source + "/Makefile") (source + "/prp_bridge.c") ]
        ++ lib.optionals withTestClients [
          (source + "/prp_test_client.c")
          (source + "/tgi_test_client.c")
        ]);
    };
    makeFlags = [ "CC=${mingw.stdenv.cc.targetPrefix}cc" ];
    buildFlags = executables;
    installPhase = ''
      runHook preInstall
      install -Dm644 -t $out/share/tobii-linux ${lib.escapeShellArgs executables}
      runHook postInstall
    '';
    meta = {
      description = "Wine-side named pipe relay between Tobii game clients and the Linux PRP service";
      platforms = lib.platforms.windows;
    };
  };

  wineRun = pkgs.stdenvNoCC.mkDerivation {
    pname = "tobii-linux-wine-run";
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = source;
      fileset = source + "/tobii-linux-wine-run";
    };
    nativeBuildInputs = [ pkgs.makeWrapper ];
    buildInputs = [ pkgs.bash ];
    installPhase = ''
      runHook preInstall
      install -Dm755 tobii-linux-wine-run $out/bin/tobii-linux-wine-run
      install -Dm644 ${prpBridge}/share/tobii-linux/prp_bridge.exe $out/share/tobii-linux/prp_bridge.exe
      wrapProgram $out/bin/tobii-linux-wine-run \
        --set-default TOBII_LINUX_BRIDGE $out/share/tobii-linux/prp_bridge.exe \
        --prefix PATH : ${lib.makeBinPath [ pkgs.coreutils pkgs.diffutils pkgs.gnugrep ]}
      runHook postInstall
    '';
    passthru.bridge = prpBridge;
    meta = {
      description = "Start a Windows game under Wine or Proton with Eye Tracker 5 head tracking";
      platforms = lib.platforms.linux;
      mainProgram = "tobii-linux-wine-run";
    };
  };
in
{
  inherit prpBridge wineRun;
}
