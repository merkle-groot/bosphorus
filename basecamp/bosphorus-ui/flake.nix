{
  description = "Bosphorus Basecamp view";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    # The input name must match metadata.json's dependency. A path flake, as the core's lib/
    # (the prebuilt FFI) isn't tracked by git.
    bosphorus.url = "path:../bosphorus-core";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
