{
  pkgs ? import <nixpkgs> { },
}:
let
  inherit (pkgs.lib) lists attrsets makeLibraryPath;

  x = pkgs.callPackage ./x { };
  inherit (x.passthru) env;
in
pkgs.mkShell {
  name = "rustc-shell";
  __structuredAttrs = true;
  strictDeps = true;

  packages = [
    pkgs.git
    pkgs.glibc.out
    pkgs.glibc.static
    x
    # Get the runtime deps of the x wrapper
  ]
  ++ lists.flatten (attrsets.attrValues env);

  env = {
    # Avoid creating text files for ICEs.
    RUSTC_ICE = 0;
    inherit (x.passthru) SSL_CERT_FILE;
    # cargo seems to dlopen libcurl, so we need it in the ld library path
    LD_LIBRARY_PATH = makeLibraryPath [
      pkgs.stdenv.cc.cc.lib
      pkgs.curl
    ];
  };
}
