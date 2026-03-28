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

  "jericho-runner" = {
    id = "jericho-runner";
    system = "x86_64-linux";
    packageAttr = "jericho-runner-incus-image";
    legacyPackageAttr = "jericho-incus-dev-image";
    defaultAlias = "jericho/dev";
    imageName = "jericho-incus-dev";
    variant = "jericho-incus-dev";
    diskSize = 12288;
    description = "Jericho Incus dev image";
    modules = [ ./jerichoci-image.nix ];
    specialArgs = { };
  };
}
