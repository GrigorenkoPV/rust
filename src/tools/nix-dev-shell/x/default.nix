{
  lib,
  stdenv,
  makeBinaryWrapper,
  # Bootstrap
  curl,
  pkg-config,
  libiconv,
  openssl,
  patchelf,
  cacert,
  zlib,
  # LLVM Deps
  ninja,
  cmake,
  callPackage,
  runCommand,
}:
let
  env = {
    cpath = [ libiconv ];
    path = [
      patchelf
      curl
      pkg-config
      cmake
      ninja
      stdenv.cc
    ];
    ldLib = [
      openssl
      zlib
      stdenv.cc.cc.lib
    ];
  };
  SSL_CERT_FILE = "${cacert}/etc/ssl/certs/ca-bundle.crt";

  bootstrap = callPackage ../../../bootstrap { };
in
runCommand "x"
  {
    __structuredAttrs = true;
    strictDeps = true;

    nativeBuildInputs = [
      makeBinaryWrapper
    ];

    passthru = {
      inherit bootstrap env SSL_CERT_FILE;
    };

    meta = {
      description = "Helper for rust-lang/rust's bootstrap";
      license = lib.licenses.mit;
      mainProgram = "x";
    };
  }
  ''
    makeWrapper ${lib.getExe bootstrap} $out/bin/x \
      --set-default SSL_CERT_FILE ${SSL_CERT_FILE} \
      --prefix CPATH ";" "${lib.makeSearchPath "include" env.cpath}" \
      --prefix PATH : ${lib.makeBinPath env.path} \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath env.ldLib}
  ''
