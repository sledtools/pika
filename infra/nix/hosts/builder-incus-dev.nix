{
  imports = [
    (import ../modules/builder.nix { enableMicrovmHost = false; })
    ../modules/incus-dev-host.nix
  ];
}
