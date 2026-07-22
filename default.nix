{ lib, rustPlatform, pkg-config, openssl, stdenv, darwin }:

rustPlatform.buildRustPackage {
  pname = "antigravity-route";
  version = "0.1.0";

  src = lib.cleanSource ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  nativeBuildInputs = [ pkg-config ];

  buildInputs = [ openssl ] ++ lib.optionals stdenv.isDarwin [
    darwin.apple_sdk.frameworks.Security
    darwin.apple_sdk.frameworks.CoreFoundation
    darwin.apple_sdk.frameworks.SystemConfiguration
  ];

  meta = with lib; {
    description = "Antigravity Route - Google Code Assist to OpenAI API proxy";
    homepage = "https://github.com/yourusername/antigravity-route";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "antigravity-route";
  };
}
