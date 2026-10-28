{
  description = "Bosphorus core module: the app API (ffi/) for Basecamp";

  # libbosphorus_ffi is prebuilt with cargo (risc0's Metal kernels need Xcode's `xcrun metal`,
  # which nixpkgs' SDK lacks) and staged into lib/ by ../build.sh. lib/ is git-ignored, so build
  # this as a path flake: `nix build path:./basecamp/bosphorus-core#lgx-portable`.
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
