# Selawik, Microsoft's open replacement for Segoe UI (SIL Open Font License 1.1), which
# Tobii's apps draw their text in. nixpkgs has no package for it; the Tobii Experience app
# asks for "Selawik Light" first (docs/experience-app.md, section 5).
#
#   selawik = import ./nix/selawik.nix { inherit pkgs; };   # $out/share/fonts/truetype/*.ttf
{ pkgs }:
pkgs.stdenvNoCC.mkDerivation {
  pname = "selawik";
  version = "1.01";
  src = pkgs.fetchurl {
    url = "https://github.com/microsoft/Selawik/releases/download/1.01/Selawik_Release.zip";
    hash = "sha256-P2LFHgXjtaHmJBz5KjcfC+LqEYOqh7MHGLvUCDKo1CM=";
  };
  nativeBuildInputs = [ pkgs.unzip ];
  sourceRoot = ".";
  installPhase = ''
    runHook preInstall
    install -Dm644 -t $out/share/fonts/truetype selawk*.ttf
    runHook postInstall
  '';
  meta = {
    description = "Microsoft's open-source Segoe UI replacement";
    homepage = "https://github.com/microsoft/Selawik";
    license = pkgs.lib.licenses.ofl;
    platforms = pkgs.lib.platforms.all;
  };
}
