# pikachat-openclaw with nix-openclaw

This repo now exposes a nix-openclaw-native deployment surface for the
`pikachat-openclaw` extension:

- `packages.<system>.pikachat`
  - the Rust daemon binary (`pikachat daemon`)
- `packages.<system>.pikachat-openclaw-extension`
  - the packaged OpenClaw extension tree
- `openclawPlugin`
  - the standard nix-openclaw plugin contract for the daemon package
- `homeManagerModules.pikachat-openclaw`
  - a small integration module that installs the extension tree into
    `~/.openclaw/extensions/pikachat-openclaw` and optionally puts `pikachat`
    on `PATH`

## Example

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
    nix-openclaw.url = "github:openclaw/nix-openclaw";
    pika.url = "github:sledtools/pika";
  };

  outputs = { nixpkgs, home-manager, nix-openclaw, pika, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ nix-openclaw.overlays.default ];
      };
    in {
      homeConfigurations.example = home-manager.lib.homeManagerConfiguration {
        inherit pkgs;
        modules = [
          nix-openclaw.homeManagerModules.openclaw
          pika.homeManagerModules.pikachat-openclaw
          {
            home.username = "example";
            home.homeDirectory = "/home/example";
            home.stateVersion = "24.11";
            programs.home-manager.enable = true;

            programs.openclaw = {
              documents = ./documents;
              pikachatOpenclaw.enable = true;

              config.channels."pikachat-openclaw" = {
                relays = [ "wss://relay.example.com" ];
                autoAcceptWelcomes = true;
                daemonBackend = "native";
              };

              instances.default = {
                enable = true;
              };
            };
          }
        ];
      };
    };
}
```

## ACP-backed daemon mode

Switch the OpenClaw channel config to ACP mode:

```nix
programs.openclaw.config.channels."pikachat-openclaw" = {
  relays = [ "wss://relay.example.com" ];
  daemonBackend = "acp";
  daemonAcpExec = "npx -y pi-acp";
  daemonAcpCwd = "/var/lib/openclaw/pikachat-acp";
};
```

## Notes

- `daemonCmd`, `daemonArgs`, and `daemonVersion` are the preferred names for
  daemon launch config.
- `sidecarCmd`, `sidecarArgs`, and `sidecarVersion` still work as compatibility
  aliases during the transition.
- The integration module is the current declarative install path for the
  extension tree itself. The `openclawPlugin` output is present for the
  standard nix-openclaw contract around the daemon package.
