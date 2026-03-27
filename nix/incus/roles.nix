{ openclawGatewayPkg, pikachatPkg }:

{
  "managed-openclaw" = {
    id = "managed-openclaw";
    system = "x86_64-linux";
    packageAttr = "managed-openclaw-incus-image";
    legacyPackageAttr = "pika-agent-incus-dev-image";
    defaultAlias = "pika-agent/dev";
    imageName = "pika-agent-incus-dev";
    variant = "managed-openclaw-incus-dev";
    diskSize = 8192;
    description = "Pika managed OpenClaw Incus dev image";
    modules = [ ./managed-agent-image.nix ];
    specialArgs = {
      inherit openclawGatewayPkg pikachatPkg;
    };
  };

  "pikaci-runner" = {
    id = "pikaci-runner";
    system = "x86_64-linux";
    packageAttr = "pikaci-runner-incus-image";
    legacyPackageAttr = "pikaci-incus-dev-image";
    defaultAlias = "pikaci/dev";
    imageName = "pikaci-incus-dev";
    variant = "pikaci-incus-dev";
    diskSize = 12288;
    description = "Pika CI Incus dev image";
    modules = [ ./pikaci-image.nix ];
    specialArgs = { };
  };
}
